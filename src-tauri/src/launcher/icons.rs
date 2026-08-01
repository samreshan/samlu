//! App icon extraction for the launcher's result list. macOS app icons live
//! as `.icns` files inside the bundle; converted to PNG via `sips` (built
//! into macOS, same "shell out to the OS tool" approach as `mdfind`/`open`)
//! and cached on disk under the app's data dir — each app's icon is only
//! ever extracted once, not on every search.

use base64::{engine::general_purpose::STANDARD, Engine};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Returns a `data:image/png;base64,...` URL for `app_path`'s icon, or
/// `None` if it can't be found/converted (the frontend just shows no icon).
pub fn icon_data_url(app_path: &str, cache_dir: &Path) -> Option<String> {
    let cache_path = cache_dir.join(cache_key(app_path));

    if !cache_path.exists() {
        extract_icon(app_path, &cache_path)?;
    }

    let bytes = std::fs::read(&cache_path).ok()?;
    Some(format!("data:image/png;base64,{}", STANDARD.encode(bytes)))
}

/// Stable, filesystem-safe cache filename for an app path — content doesn't
/// matter, only that the same app always maps to the same file.
fn cache_key(app_path: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for byte in app_path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}.png")
}

fn extract_icon(app_path: &str, out_path: &Path) -> Option<()> {
    let icon_file = bundle_icon_file(app_path)?;
    let icns_path = PathBuf::from(app_path)
        .join("Contents/Resources")
        .join(&icon_file);
    if !icns_path.exists() {
        return None;
    }

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }

    let status = Command::new("/usr/bin/sips")
        .arg("-s")
        .arg("format")
        .arg("png")
        .arg(&icns_path)
        .arg("--out")
        .arg(out_path)
        .arg("--resampleHeightWidthMax")
        .arg("64")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;

    status.success().then_some(())
}

/// Reads `CFBundleIconFile` out of the app's Info.plist via `defaults` —
/// avoids pulling in a plist-parsing crate for one field.
fn bundle_icon_file(app_path: &str) -> Option<String> {
    let info_plist = format!("{app_path}/Contents/Info");
    let output = Command::new("/usr/bin/defaults")
        .arg("read")
        .arg(&info_plist)
        .arg("CFBundleIconFile")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        return None;
    }
    Some(if name.ends_with(".icns") {
        name
    } else {
        format!("{name}.icns")
    })
}
