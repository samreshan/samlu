//! Tauri commands for the local model library in Voice settings.

use super::config::{SttEngine, VoiceConfig};
use super::download::{self, Downloads};
use super::models::{self, ModelSource};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|error| error.to_string())
}

/// Releases the resident local model if it is the one being removed.
fn unload_if_selected(config: &VoiceConfig, path: &str) {
    let stt = config.stt();
    if stt.engine == SttEngine::WhisperCpp && stt.model == path {
        super::engines::unload_local();
    }
}

#[tauri::command]
pub async fn get_voice_models(app: AppHandle) -> Result<serde_json::Value, String> {
    let data_dir = app_data_dir(&app)?;
    let config = app.state::<Arc<VoiceConfig>>().inner().clone();
    let downloads = app.state::<Arc<Downloads>>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let home = dirs::home_dir().unwrap_or_default();
        let models = models::list(&data_dir, &home, &config.added_models());
        let downloaded: HashSet<&str> = models
            .iter()
            .filter(|model| model.source == ModelSource::Downloaded)
            .map(|model| model.name.as_str())
            .collect();
        let catalog: Vec<_> = models::catalog()
            .map(|entry| {
                serde_json::json!({
                    "id": entry.id,
                    "label": entry.label,
                    "file": entry.file,
                    "kind": entry.kind,
                    "englishOnly": entry.english_only(),
                    "bytes": entry.bytes(),
                    "downloaded": downloaded.contains(entry.file),
                    "downloading": downloads.is_active(entry.id),
                })
            })
            .collect();
        // A cloud engine's model name is not a local model; report no selection.
        let stt = config.stt();
        let selected = if stt.engine == SttEngine::WhisperCpp {
            stt.model
        } else {
            String::new()
        };
        let preparing =
            super::engines::bark::loading().map(|path| path.to_string_lossy().into_owned());
        serde_json::json!({
            "models": models,
            "catalog": catalog,
            "preparing": preparing,
            "recommended": models::recommended_id(),
            "selected": selected,
        })
    })
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn voice_download_model(app: AppHandle, id: String) -> Result<String, String> {
    let entry = models::catalog_entry(&id).ok_or_else(|| "Unknown model.".to_string())?;
    let dir = models::models_dir(&app_data_dir(&app)?);
    let downloads = app.state::<Arc<Downloads>>().inner().clone();
    let path = download::download(&app, entry, &dir, &downloads).await?;
    if entry.kind == models::ModelKind::Bark {
        // Core ML's first load takes minutes; do it now rather than at the
        // first dictation, then keep the model only if it is the one in use.
        let config = app.state::<Arc<VoiceConfig>>().inner().clone();
        let prepared = path.clone();
        std::thread::spawn(move || {
            if let Err(error) = super::engines::bark::prepare(&prepared) {
                log::warn!("[voice] Underdog Bark could not be prepared: {error}");
            }
            let stt = config.stt();
            let in_use = if stt.engine == SttEngine::WhisperCpp {
                stt.model
            } else {
                String::new()
            };
            super::engines::bark::keep_only(Path::new(&in_use));
        });
    }
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn voice_cancel_model_download(id: String, downloads: tauri::State<'_, Arc<Downloads>>) {
    downloads.cancel(&id);
}

#[tauri::command]
pub async fn voice_add_model(app: AppHandle) -> Result<Option<String>, String> {
    let Some(picked) = app
        .dialog()
        .file()
        .set_title("Choose a whisper.cpp model")
        .add_filter("whisper.cpp model", &["bin"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|error| error.to_string())?;
    models::inspect(&path)?;
    let path = path.to_string_lossy().into_owned();
    app.state::<Arc<VoiceConfig>>().add_model(path.clone());
    Ok(Some(path))
}

#[tauri::command]
pub fn voice_remove_model(path: String, config: tauri::State<'_, Arc<VoiceConfig>>) {
    unload_if_selected(&config, &path);
    super::engines::bark::release(Path::new(&path));
    config.remove_model(&path);
}

#[tauri::command]
pub fn voice_delete_model(
    app: AppHandle,
    path: String,
    config: tauri::State<'_, Arc<VoiceConfig>>,
) -> Result<(), String> {
    unload_if_selected(&config, &path);
    super::engines::bark::release(Path::new(&path));
    download::delete_downloaded(&models::models_dir(&app_data_dir(&app)?), Path::new(&path))
}
