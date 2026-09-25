#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use tauri::{Manager, menu::{Menu, MenuItem}, tray::TrayIconBuilder};

#[tauri::command]
fn set_pinned(window: tauri::WebviewWindow, pinned: bool) -> Result<(), String> {
    window.set_always_on_top(pinned).map_err(|error| error.to_string())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![set_pinned])
        .setup(|app| {
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
                    "show" => if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    },
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Only suppress closing after a successful hide.
                if window.hide().is_ok() { api.prevent_close(); }
            }
        })
        .run(tauri::generate_context!())
        .expect("Unable to run Luma");
}
