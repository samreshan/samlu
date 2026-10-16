//! Software updates. Samlu reads a signed release feed (`latest.json` on the
//! newest GitHub release), downloads a newer build, verifies its signature
//! against the public key compiled into this app, and swaps the app bundle in
//! place. The running copy keeps working; the new version starts the next time
//! Samlu does, which the person can trigger with "Restart to Update".
//!
//! Background checks run every few hours while automatic checking is on. With
//! automatic installation also on, an update found in the background is
//! installed straight away, but only when that needs no administrator
//! password — a surprise password prompt is worse than a notification.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::menu::MenuItem;
use tauri::{AppHandle, Emitter, Manager, Wry};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::settings::AppConfig;

pub const STATE_EVENT: &str = "updater://state";
/// Give startup, the hook server and the overlays room before the network.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(45);
const CHECK_INTERVAL: chrono::Duration = chrono::Duration::hours(6);
/// After a failed check — usually no network — try again sooner.
const RETRY_INTERVAL: chrono::Duration = chrono::Duration::minutes(30);
/// How often the background loop wakes to see whether a check is due. Waking
/// on a short tick rather than sleeping six hours keeps checks on schedule
/// across sleep and wake.
const TICK: Duration = Duration::from_secs(10 * 60);
/// Download progress is reported to Settings at most this often.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Installing,
    Ready,
    Error,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableUpdate {
    pub version: String,
    pub notes: Option<String>,
    pub date: Option<String>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub received: u64,
    pub total: Option<u64>,
}

/// Everything Settings and the tray need to describe updates.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSnapshot {
    pub current_version: String,
    pub status: Status,
    pub available: Option<AvailableUpdate>,
    pub progress: Option<Progress>,
    pub error: Option<String>,
    pub last_checked_at: Option<String>,
    pub auto_check: bool,
    pub auto_install: bool,
    /// Why this copy of Samlu cannot update itself, when it cannot.
    pub install_blocker: Option<String>,
    /// Signed with a Developer ID. Ad-hoc signed builds lose their macOS
    /// privacy permissions on every update, and Settings says so.
    pub signed_build: bool,
}

struct Inner {
    status: Status,
    available: Option<AvailableUpdate>,
    pending: Option<Update>,
    progress: Option<Progress>,
    error: Option<String>,
    /// The last notification sent, as "available:0.2.5" or "ready:0.2.5", so
    /// a background check every six hours does not repeat itself.
    notified: Option<String>,
}

pub struct UpdaterState {
    inner: Mutex<Inner>,
    menu_item: Mutex<Option<MenuItem<Wry>>>,
    signed_build: bool,
}

impl UpdaterState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                status: Status::Idle,
                available: None,
                pending: None,
                progress: None,
                error: None,
                notified: None,
            }),
            menu_item: Mutex::new(None),
            signed_build: crate::platform::is_developer_id_signed(),
        }
    }

    /// The tray's update item, relabelled as the update moves along.
    pub fn set_menu_item(&self, item: MenuItem<Wry>) {
        *self.menu_item.lock().unwrap() = Some(item);
    }

    pub fn is_ready(&self) -> bool {
        self.status() == Status::Ready
    }

    fn status(&self) -> Status {
        self.inner.lock().unwrap().status
    }

    fn is_busy(&self) -> bool {
        matches!(
            self.inner.lock().unwrap().status,
            Status::Checking | Status::Downloading | Status::Installing
        )
    }
}

impl Default for UpdaterState {
    fn default() -> Self {
        Self::new()
    }
}

/// Starts the background schedule. Called once from setup.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(first_check_delay());
        loop {
            if check_is_due(&app) {
                tauri::async_runtime::block_on(run_check(&app, false));
            }
            std::thread::sleep(TICK);
        }
    });
}

fn first_check_delay() -> Duration {
    // Debug builds can shorten the wait to exercise the flow end to end.
    #[cfg(debug_assertions)]
    if let Some(seconds) = std::env::var("SAMLU_UPDATER_FIRST_CHECK_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
    {
        return Duration::from_secs(seconds);
    }
    FIRST_CHECK_DELAY
}

fn check_is_due(app: &AppHandle) -> bool {
    let config = app.state::<Arc<AppConfig>>();
    let state = app.state::<Arc<UpdaterState>>();
    if !config.update_auto_check() || state.is_busy() || state.is_ready() {
        return false;
    }
    let interval = if state.status() == Status::Error {
        RETRY_INTERVAL
    } else {
        CHECK_INTERVAL
    };
    match config
        .update_last_checked_at()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
    {
        Some(last) => chrono::Utc::now().signed_duration_since(last) >= interval,
        None => true,
    }
}

/// The tray's "Check for Updates…" and the launcher command: show the result
/// in Settings, or restart if an update is already waiting.
pub fn check_from_menu(app: &AppHandle) {
    if app.state::<Arc<UpdaterState>>().is_ready() {
        app.restart();
    }
    crate::tray::show_main(app);
    let _ = app.emit_to(crate::lifecycle::MAIN_LABEL, "settings://tab", "general");
    let app = app.clone();
    tauri::async_runtime::spawn(async move { run_check(&app, true).await });
}

pub fn snapshot(app: &AppHandle) -> UpdateSnapshot {
    let config = app.state::<Arc<AppConfig>>();
    let state = app.state::<Arc<UpdaterState>>();
    let inner = state.inner.lock().unwrap();
    UpdateSnapshot {
        current_version: app.package_info().version.to_string(),
        status: inner.status,
        available: inner.available.clone(),
        progress: inner.progress,
        error: inner.error.clone(),
        last_checked_at: config.update_last_checked_at(),
        auto_check: config.update_auto_check(),
        auto_install: config.update_auto_install(),
        install_blocker: install_blocker(),
        signed_build: state.signed_build,
    }
}

fn publish(app: &AppHandle) {
    let snapshot = snapshot(app);
    relabel_menu_item(app, &snapshot);
    let _ = app.emit(STATE_EVENT, snapshot);
}

fn relabel_menu_item(app: &AppHandle, snapshot: &UpdateSnapshot) {
    let state = app.state::<Arc<UpdaterState>>();
    let item = state.menu_item.lock().unwrap();
    let Some(item) = item.as_ref() else {
        return;
    };
    let label = match (&snapshot.status, &snapshot.available) {
        (Status::Ready, Some(update)) => format!("Restart to Update to {}", update.version),
        (Status::Ready, None) => "Restart to Update".to_string(),
        (Status::Downloading | Status::Installing, _) => "Downloading Update…".to_string(),
        (Status::Checking, _) => "Checking for Updates…".to_string(),
        _ => "Check for Updates…".to_string(),
    };
    let _ = item.set_text(label);
    let _ = item.set_enabled(!matches!(
        snapshot.status,
        Status::Checking | Status::Downloading | Status::Installing
    ));
}

fn set_status(app: &AppHandle, change: impl FnOnce(&mut Inner)) {
    {
        let state = app.state::<Arc<UpdaterState>>();
        let mut inner = state.inner.lock().unwrap();
        change(&mut inner);
    }
    publish(app);
}

/// Looks for a newer release. A manual check always reports back in Settings;
/// a background one stays quiet unless something is worth a notification.
pub async fn run_check(app: &AppHandle, manual: bool) {
    {
        let state = app.state::<Arc<UpdaterState>>();
        let mut inner = state.inner.lock().unwrap();
        if matches!(
            inner.status,
            Status::Checking | Status::Downloading | Status::Installing | Status::Ready
        ) {
            return;
        }
        inner.status = Status::Checking;
        inner.error = None;
    }
    publish(app);

    let result = match build_updater(app) {
        Ok(updater) => updater.check().await.map_err(describe_error),
        Err(error) => Err(error),
    };
    app.state::<Arc<AppConfig>>()
        .set_update_last_checked_at(chrono::Utc::now().to_rfc3339());

    match result {
        Ok(Some(update)) => {
            let available = AvailableUpdate {
                version: update.version.clone(),
                notes: update
                    .body
                    .clone()
                    .map(|notes| notes.trim().to_string())
                    .filter(|notes| !notes.is_empty()),
                date: update.date.and_then(|date| {
                    chrono::DateTime::from_timestamp(date.unix_timestamp(), 0)
                        .map(|date| date.to_rfc3339())
                }),
            };
            log::info!(
                "update {} is available (running {})",
                available.version,
                update.current_version
            );
            set_status(app, |inner| {
                inner.status = Status::Available;
                inner.available = Some(available.clone());
                inner.pending = Some(update);
            });

            if already_installed(&available.version) {
                set_status(app, |inner| inner.status = Status::Ready);
                return;
            }
            let config = app.state::<Arc<AppConfig>>();
            let automatic = config.update_auto_check() && config.update_auto_install();
            let can_install = install_blocker().is_none() && (manual || can_replace_silently());
            if automatic && can_install {
                run_install(app).await;
            } else if !manual {
                notify_once(
                    app,
                    format!("available:{}", available.version),
                    &format!("Samlu {} is available", available.version),
                    "Open Samlu Settings to install it.",
                );
            }
        }
        Ok(None) => set_status(app, |inner| {
            inner.status = Status::UpToDate;
            inner.available = None;
            inner.pending = None;
        }),
        Err(error) => {
            log::warn!("update check failed: {error}");
            set_status(app, |inner| {
                inner.status = Status::Error;
                inner.error = Some(error);
            });
        }
    }
}

/// Downloads, verifies and installs the update found by the last check.
pub async fn run_install(app: &AppHandle) {
    let update = {
        let state = app.state::<Arc<UpdaterState>>();
        let mut inner = state.inner.lock().unwrap();
        if matches!(
            inner.status,
            Status::Checking | Status::Downloading | Status::Installing | Status::Ready
        ) {
            return;
        }
        let Some(update) = inner.pending.clone() else {
            return;
        };
        inner.status = Status::Downloading;
        inner.progress = Some(Progress {
            received: 0,
            total: None,
        });
        inner.error = None;
        update
    };
    publish(app);

    if let Some(blocker) = install_blocker() {
        set_status(app, |inner| {
            inner.status = Status::Error;
            inner.progress = None;
            inner.error = Some(blocker);
        });
        return;
    }

    let mut received = 0u64;
    let mut last_report = Instant::now();
    let progress_app = app.clone();
    let downloaded = update
        .download(
            |chunk, total| {
                received += chunk as u64;
                if last_report.elapsed() < PROGRESS_INTERVAL {
                    return;
                }
                last_report = Instant::now();
                let progress = Progress { received, total };
                set_status(&progress_app, |inner| inner.progress = Some(progress));
            },
            || {},
        )
        .await;

    let bytes = match downloaded {
        Ok(bytes) => bytes,
        Err(error) => {
            let error = describe_error(error);
            log::warn!("update download failed: {error}");
            set_status(app, |inner| {
                inner.status = Status::Error;
                inner.progress = None;
                inner.error = Some(error);
            });
            return;
        }
    };

    set_status(app, |inner| {
        inner.status = Status::Installing;
        inner.progress = None;
    });
    let version = update.version.clone();
    let installed = tauri::async_runtime::spawn_blocking(move || update.install(bytes)).await;
    match installed {
        Ok(Ok(())) => {
            log::info!("installed update {version}; it starts with the next launch");
            set_status(app, |inner| inner.status = Status::Ready);
            notify_once(
                app,
                format!("ready:{version}"),
                &format!("Samlu {version} is ready"),
                "Restart Samlu to finish updating.",
            );
        }
        Ok(Err(error)) => {
            let error = describe_error(error);
            log::warn!("update install failed: {error}");
            set_status(app, |inner| {
                inner.status = Status::Error;
                inner.error = Some(error);
            });
        }
        Err(error) => set_status(app, |inner| {
            inner.status = Status::Error;
            inner.error = Some(format!("The installer stopped unexpectedly: {error}"));
        }),
    }
}

fn build_updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    #[allow(unused_mut)]
    let mut builder = app.updater_builder().timeout(Duration::from_secs(30));
    // Debug builds can point at a local feed to test the whole flow; release
    // builds only ever read the feed compiled into tauri.conf.json.
    #[cfg(debug_assertions)]
    if let Ok(endpoint) = std::env::var("SAMLU_UPDATER_ENDPOINT") {
        let url: tauri::Url = endpoint
            .parse()
            .map_err(|error| format!("bad SAMLU_UPDATER_ENDPOINT: {error}"))?;
        builder = builder.endpoints(vec![url]).map_err(describe_error)?;
    }
    builder.build().map_err(describe_error)
}

fn describe_error(error: tauri_plugin_updater::Error) -> String {
    use tauri_plugin_updater::Error;
    match error {
        Error::Reqwest(_) | Error::Network(_) => {
            "Samlu couldn't reach the update server. Check your connection and try again."
                .to_string()
        }
        Error::ReleaseNotFound => "No update information has been published yet.".to_string(),
        Error::TargetNotFound(_) | Error::TargetsNotFound(_) => {
            "The latest release has no build for this Mac.".to_string()
        }
        Error::Minisign(_) | Error::Base64(_) | Error::SignatureUtf8(_) => {
            "The downloaded update failed its signature check, so it was not installed.".to_string()
        }
        other => other.to_string(),
    }
}

fn notify_once(app: &AppHandle, key: String, title: &str, body: &str) {
    {
        let state = app.state::<Arc<UpdaterState>>();
        let mut inner = state.inner.lock().unwrap();
        if inner.notified.as_deref() == Some(key.as_str()) {
            return;
        }
        inner.notified = Some(key);
    }
    let _ = app.notification().builder().title(title).body(body).show();
}

/// The `.app` this process runs from, if it runs from one.
fn current_bundle() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let is_bundle = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app";
    is_bundle.then(|| bundle.to_path_buf())
}

/// Why this copy cannot install updates at all. `None` means it can, possibly
/// after an administrator password.
#[cfg(not(target_os = "macos"))]
pub fn install_blocker() -> Option<String> {
    // The Windows installer updates Samlu wherever it was installed. It also
    // closes Samlu while it runs, which is why there is no bundle to swap
    // silently and updates only install when asked.
    None
}

/// Why this copy cannot install updates at all. `None` means it can, possibly
/// after an administrator password.
#[cfg(target_os = "macos")]
pub fn install_blocker() -> Option<String> {
    let Some(bundle) = current_bundle() else {
        return Some(
            "Updates install into Samlu.app, and this copy is not running from one.".into(),
        );
    };
    let path = bundle.to_string_lossy();
    if path.contains("/AppTranslocation/") {
        return Some(
            "macOS is running Samlu from a temporary location. Move Samlu to Applications and open it from there to receive updates."
                .into(),
        );
    }
    // Only a copy on another volume can be stuck read-only (a mounted disk
    // image); probing anywhere else would write a file on every call.
    if path.starts_with("/Volumes/") && bundle.parent().is_some_and(|folder| !is_writable(folder)) {
        return Some(
            "Samlu is running from its disk image. Drag it into Applications to receive updates."
                .into(),
        );
    }
    None
}

/// Whether the bundle can be swapped without asking for a password.
fn can_replace_silently() -> bool {
    current_bundle()
        .and_then(|bundle| bundle.parent().map(is_writable))
        .unwrap_or(false)
}

fn is_writable(directory: &Path) -> bool {
    let probe = directory.join(format!(".samlu-update-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// True when the bundle on disk is already at least `version`, as when an
/// update was installed but Samlu has not restarted since.
fn already_installed(version: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct Info {
        #[serde(rename = "CFBundleShortVersionString")]
        version: String,
    }
    let Some(bundle) = current_bundle() else {
        return false;
    };
    let Ok(info) = plist::from_file::<_, Info>(bundle.join("Contents/Info.plist")) else {
        return false;
    };
    match (
        semver::Version::parse(&info.version),
        semver::Version::parse(version),
    ) {
        (Ok(on_disk), Ok(wanted)) => on_disk >= wanted,
        _ => false,
    }
}

#[tauri::command]
pub fn get_update_state(app: AppHandle) -> UpdateSnapshot {
    snapshot(&app)
}

#[tauri::command]
pub fn set_update_preferences(
    app: AppHandle,
    auto_check: bool,
    auto_install: bool,
) -> UpdateSnapshot {
    let config = app.state::<Arc<AppConfig>>();
    config.set_update_preferences(auto_check, auto_install);
    // Turning automatic installs on while an update is waiting applies it now.
    let waiting = {
        let state = app.state::<Arc<UpdaterState>>();
        let inner = state.inner.lock().unwrap();
        inner.status == Status::Available
    };
    let automatic = auto_check && auto_install;
    if automatic && waiting && install_blocker().is_none() && can_replace_silently() {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { run_install(&app).await });
    }
    publish(&app);
    snapshot(&app)
}

#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> UpdateSnapshot {
    run_check(&app, true).await;
    snapshot(&app)
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> UpdateSnapshot {
    run_install(&app).await;
    snapshot(&app)
}

#[tauri::command]
pub fn restart_to_update(app: AppHandle) {
    app.restart();
}
