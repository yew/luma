#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod github;
mod history;
mod network;
mod preferences;
mod storage;
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager,
};
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
fn open_github_verification(app: tauri::AppHandle) -> Result<(), String> {
    app.opener()
        .open_url("https://github.com/login/device", None::<&str>)
        .map_err(|_| "Unable to open GitHub in the system browser".to_string())
}

fn epoch_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            open_github_verification,
            github::github_status,
            github::github_begin,
            github::github_poll,
            github::github_cancel,
            github::github_disconnect,
            github::github_refresh,
            preferences::get_preferences,
            preferences::update_preferences,
            preferences::set_compact_view,
            history::history_settings,
            history::update_history_settings,
            history::query_usage_history,
            history::rebuild_usage_cache,
            history::clear_usage_history,
            history::clear_usage_cache
        ])
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let storage = Arc::new(storage::Storage::open(data_dir.join("usage.sqlite3"))?);
            storage.prune(epoch_millis())?;
            let preferences = preferences::PreferencesState::initialize(app.handle())?;
            let proxy = preferences.snapshot()?.proxy;
            app.manage(github::GitHubService::new(storage.clone(), &proxy).map_err(|e| e.message)?);
            app.manage(storage);
            app.manage(preferences);
            preferences::restore_window(app.handle())?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
                let mut cleanup_at = std::time::Instant::now();
                loop {
                    tick.tick().await;
                    if cleanup_at.elapsed() >= std::time::Duration::from_secs(3600) {
                        let _ = handle
                            .state::<Arc<storage::Storage>>()
                            .prune(epoch_millis());
                        cleanup_at = std::time::Instant::now();
                    }
                    handle
                        .state::<github::GitHubService>()
                        .background_tick(&handle)
                        .await;
                }
            });
            let show = MenuItem::with_id(app, "show", "Show Luma", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Luma", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut pixels = vec![0_u8; 16 * 16 * 4];
            for y in 0_i32..16 {
                for x in 0_i32..16 {
                    if (x - 8).abs() + (y - 8).abs() <= 6 {
                        let index = ((y * 16 + x) * 4) as usize;
                        pixels[index..index + 4].copy_from_slice(&[150, 169, 255, 255]);
                    }
                }
            }
            TrayIconBuilder::new()
                .icon(tauri::image::Image::new_owned(pixels, 16, 16))
                .tooltip("Luma")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        let _ = preferences::restore_window(app);
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            preferences::persist_window_event(window, event);
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Only suppress closing after a successful hide.
                if window.hide().is_ok() {
                    api.prevent_close();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("Unable to run Luma");
}
