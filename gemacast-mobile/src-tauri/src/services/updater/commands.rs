use gemacast_core::updater::{
    AvailableUpdate, UpdateArtifact, UpdateCache, UpdateChecker, UpdateDownloader,
};
use tauri::{Emitter, Manager};
use tokio::sync::mpsc;

/// Check whether an update is available.
///
/// Returns `Some(AvailableUpdate)` when a newer version exists, or `None` when
/// the app is already up-to-date.
#[tauri::command]
pub async fn check_for_update(app: tauri::AppHandle) -> Result<Option<AvailableUpdate>, String> {
    let current_version = app.config().version.clone().unwrap_or_default();
    UpdateChecker::latest_release()
        .find_available_update(&current_version, "android")
        .await
        .map_err(|error| error.to_string())
}

/// Download the update APK to the app's cache directory.
///
/// Emits `update-progress` events to the frontend with the download percentage.
/// Returns the absolute path to the downloaded APK file.
///
/// `sha256` is required.
#[tauri::command]
pub async fn download_update(
    app: tauri::AppHandle,
    url: String,
    sha256: String,
) -> Result<String, String> {
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("Failed to get cache dir: {e}"))?
        .join("updates");

    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("Failed to create updates dir: {e}"))?;

    let file_path = cache_dir.join("gemacast-update.apk");

    let (progress_tx, mut progress_rx) = mpsc::channel::<u8>(32);

    let app_handle = app.clone();
    tokio::spawn(async move {
        while let Some(percent) = progress_rx.recv().await {
            let _ = app_handle.emit("update-progress", percent);
        }
    });

    let artifact = UpdateArtifact::new(url, sha256);
    UpdateDownloader::default()
        .download(&artifact, &file_path, Some(progress_tx))
        .await
        .map_err(|error| error.to_string())?;

    file_path
        .to_str()
        .map(String::from)
        .ok_or_else(|| "Path contains invalid UTF-8".to_string())
}

/// Clean up any previously downloaded update APKs from the cache.
///
/// Should be called on app startup to reclaim space from stale downloads.
#[tauri::command]
pub async fn cleanup_stale_updates(app: tauri::AppHandle) -> Result<(), String> {
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("Failed to get cache dir: {e}"))?
        .join("updates");

    UpdateCache::at(&cache_dir)
        .remove_stale_files()
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Trigger the Android system installer for the downloaded APK.
///
/// On non-Android platforms this is a no-op that returns an error.
#[tauri::command]
pub async fn install_apk(app: tauri::AppHandle, path: String) -> Result<(), String> {
    use tauri::Manager;

    let updates_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("Failed to get cache dir: {e}"))?
        .join("updates");
    let expected_path = updates_dir.join("gemacast-update.apk");
    let requested_path = std::path::Path::new(&path)
        .canonicalize()
        .map_err(|e| format!("Update APK path is unavailable: {e}"))?;
    let expected_path = expected_path
        .canonicalize()
        .map_err(|e| format!("Verified update APK is unavailable: {e}"))?;
    if requested_path != expected_path {
        return Err("Refusing to install an APK outside the verified update cache".into());
    }

    crate::services::platform::PlatformFacade::new(app)
        .install_apk(expected_path.to_str().ok_or("Update APK path is not valid UTF-8")?)
        .map_err(|error| error.to_string())
}
