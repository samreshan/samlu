//! Tauri commands for the whisper model library in Voice settings.

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

/// Releases the resident whisper model if it is the one being removed.
fn unload_if_selected(config: &VoiceConfig, path: &str) {
    let stt = config.stt();
    if stt.engine == SttEngine::WhisperCpp && stt.model == path {
        super::engines::whisper::unload();
    }
}

#[tauri::command]
pub fn get_voice_models(
    app: AppHandle,
    config: tauri::State<'_, Arc<VoiceConfig>>,
    downloads: tauri::State<'_, Arc<Downloads>>,
) -> Result<serde_json::Value, String> {
    let home = dirs::home_dir().unwrap_or_default();
    let models = models::list(&app_data_dir(&app)?, &home, &config.added_models());
    let downloaded: HashSet<&str> = models
        .iter()
        .filter(|model| model.source == ModelSource::Downloaded)
        .map(|model| model.name.as_str())
        .collect();
    let catalog: Vec<_> = models::CATALOG
        .iter()
        .map(|entry| {
            serde_json::json!({
                "id": entry.id,
                "label": entry.label,
                "file": entry.file,
                "bytes": entry.bytes,
                "downloaded": downloaded.contains(entry.file),
                "downloading": downloads.is_active(entry.id),
            })
        })
        .collect();
    // A cloud engine's model name is not a whisper file; report no selection.
    let stt = config.stt();
    let selected = if stt.engine == SttEngine::WhisperCpp {
        stt.model
    } else {
        String::new()
    };
    Ok(serde_json::json!({
        "models": models,
        "catalog": catalog,
        "recommended": models::recommended_id(),
        "selected": selected,
    }))
}

#[tauri::command]
pub async fn voice_download_model(app: AppHandle, id: String) -> Result<String, String> {
    let entry = models::catalog_entry(&id).ok_or_else(|| "Unknown model.".to_string())?;
    let dir = models::models_dir(&app_data_dir(&app)?);
    let downloads = app.state::<Arc<Downloads>>().inner().clone();
    let path = download::download(&app, entry, &dir, &downloads).await?;
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
    config.remove_model(&path);
}

#[tauri::command]
pub fn voice_delete_model(
    app: AppHandle,
    path: String,
    config: tauri::State<'_, Arc<VoiceConfig>>,
) -> Result<(), String> {
    unload_if_selected(&config, &path);
    download::delete_downloaded(&models::models_dir(&app_data_dir(&app)?), Path::new(&path))
}
