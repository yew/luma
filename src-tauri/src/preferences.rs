use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};
use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize};
use tauri_plugin_autostart::ManagerExt;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub refresh_interval_secs: u32,
    pub launch_at_login: bool,
    pub hide_titles: bool,
    pub hide_paths: bool,
    pub pinned: bool,
    pub collapsed: bool,
    pub proxy: crate::network::ProxySettings,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            refresh_interval_secs: 300,
            launch_at_login: false,
            hide_titles: false,
            hide_paths: false,
            pinned: true,
            collapsed: false,
            proxy: crate::network::ProxySettings::default(),
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreferencesPatch {
    pub refresh_interval_secs: Option<u32>,
    pub launch_at_login: Option<bool>,
    pub hide_titles: Option<bool>,
    pub hide_paths: Option<bool>,
    pub pinned: Option<bool>,
    pub collapsed: Option<bool>,
    pub proxy: Option<crate::network::ProxySettings>,
}

impl Preferences {
    fn patched(&self, patch: &PreferencesPatch) -> Result<Self, String> {
        let mut next = self.clone();
        if let Some(interval) = patch.refresh_interval_secs {
            if !(60..=3_600).contains(&interval) {
                return Err("Refresh interval must be between 60 and 3600 seconds.".into());
            }
            next.refresh_interval_secs = interval;
        }
        if let Some(value) = patch.launch_at_login {
            next.launch_at_login = value;
        }
        if let Some(value) = patch.hide_titles {
            next.hide_titles = value;
        }
        if let Some(value) = patch.hide_paths {
            next.hide_paths = value;
        }
        if let Some(value) = patch.pinned {
            next.pinned = value;
        }
        if let Some(value) = patch.collapsed {
            next.collapsed = value;
        }
        if let Some(proxy) = &patch.proxy {
            next.proxy = proxy.validated()?;
        }
        Ok(next)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct WindowGeometry {
    x: i32,
    y: i32,
    // Inner dimensions remain logical pixels when moving between DPI scales.
    width: f64,
    height: f64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct SavedPreferences {
    preferences: Preferences,
    geometry: Option<WindowGeometry>,
}

struct Store {
    connection: Connection,
    saved: SavedPreferences,
}

impl Store {
    fn open(path: &Path) -> Result<Self, String> {
        let connection = Connection::open(path).map_err(|_| "Unable to open local preferences.")?;
        connection
            .busy_timeout(Duration::from_secs(3))
            .map_err(|_| "Unable to configure local preferences.")?;
        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|_| "Unable to read preferences schema.")?;
        if version > 1 {
            return Err("Preferences were saved by a newer Luma version.".into());
        }
        connection.execute_batch(
            "BEGIN; CREATE TABLE IF NOT EXISTS preferences (id INTEGER PRIMARY KEY CHECK (id = 1), value TEXT NOT NULL); PRAGMA user_version = 1; COMMIT;",
        ).map_err(|_| "Unable to initialize local preferences.")?;
        let json: Option<String> = connection
            .query_row("SELECT value FROM preferences WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|_| "Unable to read local preferences.")?;
        let saved = match json {
            Some(value) => serde_json::from_str::<SavedPreferences>(&value)
                .map_err(|_| "Local preferences are invalid.")?,
            None => SavedPreferences::default(),
        };
        saved.preferences.proxy.validated()?;
        saved.preferences.patched(&PreferencesPatch {
            refresh_interval_secs: Some(saved.preferences.refresh_interval_secs),
            ..Default::default()
        })?;
        Ok(Self { connection, saved })
    }

    fn persist(&self) -> Result<(), String> {
        let json = serde_json::to_string(&self.saved)
            .map_err(|_| "Unable to encode local preferences.")?;
        self.connection.execute(
            "INSERT INTO preferences (id, value) VALUES (1, ?1) ON CONFLICT(id) DO UPDATE SET value = excluded.value",
            params![json],
        ).map_err(|_| "Unable to save local preferences.")?;
        Ok(())
    }
}

pub struct PreferencesState {
    store: Arc<Mutex<Store>>,
    updates: tokio::sync::Mutex<()>,
    geometry_changed: mpsc::Sender<()>,
    compact_view: AtomicBool,
}

impl PreferencesState {
    pub fn initialize(app: &tauri::AppHandle) -> Result<Self, String> {
        let directory: PathBuf = app
            .path()
            .app_data_dir()
            .map_err(|_| "Unable to locate local app storage.")?;
        std::fs::create_dir_all(&directory).map_err(|_| "Unable to create local app storage.")?;
        let mut store = Store::open(&directory.join("preferences.sqlite3"))?;
        // Respect changes made in the OS login-item settings between Luma runs.
        store.saved.preferences.launch_at_login = app
            .autolaunch()
            .is_enabled()
            .map_err(|_| "Unable to read launch-at-login settings.")?;
        store.persist()?;
        let store = Arc::new(Mutex::new(store));
        let (sender, receiver) = mpsc::channel();
        let background_store = Arc::clone(&store);
        let app = app.clone();
        std::thread::Builder::new()
            .name("luma-window-preferences".into())
            .spawn(move || {
                while receiver.recv().is_ok() {
                    // Coalesce move/resize bursts without writing SQLite on the UI thread.
                    while receiver.recv_timeout(Duration::from_millis(350)).is_ok() {}
                    let result = background_store
                        .lock()
                        .map_err(|_| "Unable to access local preferences.".to_string())
                        .and_then(|store| store.persist());
                    if let Err(message) = result {
                        let _ = app.emit("preferences-error", message);
                    }
                }
            })
            .map_err(|_| "Unable to start window preference persistence.")?;
        Ok(Self {
            store,
            updates: tokio::sync::Mutex::new(()),
            geometry_changed: sender,
            compact_view: AtomicBool::new(false),
        })
    }

    pub fn snapshot(&self) -> Result<Preferences, String> {
        self.store
            .lock()
            .map(|store| store.saved.preferences.clone())
            .map_err(|_| "Unable to access local preferences.".into())
    }
}

pub fn current_refresh_interval(app: &tauri::AppHandle) -> u64 {
    app.try_state::<PreferencesState>()
        .and_then(|state| state.snapshot().ok())
        .map(|preferences| u64::from(preferences.refresh_interval_secs))
        .unwrap_or(300)
}

#[tauri::command]
pub fn get_preferences(state: tauri::State<'_, PreferencesState>) -> Result<Preferences, String> {
    state.snapshot()
}

#[tauri::command]
pub async fn update_preferences(
    app: tauri::AppHandle,
    state: tauri::State<'_, PreferencesState>,
    patch: PreferencesPatch,
) -> Result<Preferences, String> {
    let _update = state.updates.lock().await;
    let previous = state.snapshot()?;
    let next = previous.patched(&patch)?;
    let client = if next.proxy != previous.proxy {
        Some(crate::network::build_client(&next.proxy)?)
    } else {
        None
    };
    let window = app
        .get_webview_window("main")
        .ok_or("Luma's window is unavailable.")?;
    if next.pinned != previous.pinned {
        window
            .set_always_on_top(next.pinned)
            .map_err(|_| "Unable to change window pinning.")?;
    }
    if next.launch_at_login != previous.launch_at_login {
        let result = if next.launch_at_login {
            app.autolaunch().enable()
        } else {
            app.autolaunch().disable()
        };
        if result.is_err() {
            let _ = window.set_always_on_top(previous.pinned);
            return Err(
                "Unable to change launch at login. Check system login-item permissions.".into(),
            );
        }
    }
    let persist = || {
        let mut store = state
            .store
            .lock()
            .map_err(|_| "Unable to access local preferences.")?;
        store.saved.preferences = next.clone();
        if let Err(error) = store.persist() {
            store.saved.preferences = previous.clone();
            return Err(error);
        }
        Ok(())
    };
    let saved = if let Some(client) = client {
        app.state::<crate::github::GitHubService>()
            .replace_client(client, persist)
    } else {
        persist()
    };
    if let Err(error) = saved {
        let _ = window.set_always_on_top(previous.pinned);
        if next.launch_at_login != previous.launch_at_login {
            let _ = if previous.launch_at_login {
                app.autolaunch().enable()
            } else {
                app.autolaunch().disable()
            };
        }
        return Err(error);
    }
    let _ = app.emit("preferences-changed", &next);
    Ok(next)
}

/// Change the visible layout without overwriting expanded window dimensions.
/// The collapsed preference is persisted separately; Settings can temporarily expand.
#[tauri::command]
pub fn set_compact_view(app: tauri::AppHandle, compact: bool) -> Result<(), String> {
    let state = app.state::<PreferencesState>();
    let window = app
        .get_webview_window("main")
        .ok_or("Luma's window is unavailable.")?;
    if state.compact_view.load(Ordering::Relaxed) == compact {
        return Ok(());
    }
    let scale = window
        .scale_factor()
        .map_err(|_| "Unable to read window scale.")?;
    let size = window
        .inner_size()
        .map_err(|_| "Unable to read window size.")?;
    let position = window
        .outer_position()
        .map_err(|_| "Unable to read window position.")?;
    let expanded = {
        let mut store = state
            .store
            .lock()
            .map_err(|_| "Unable to access local preferences.")?;
        if compact {
            store.saved.geometry = Some(WindowGeometry {
                x: position.x,
                y: position.y,
                width: f64::from(size.width) / scale,
                height: f64::from(size.height) / scale,
            });
            store.persist()?;
        }
        store.saved.geometry.unwrap_or(WindowGeometry {
            x: position.x,
            y: position.y,
            width: 360.0,
            height: 480.0,
        })
    };
    state.compact_view.store(compact, Ordering::Relaxed);
    let height = if compact { 240.0 } else { expanded.height };
    if window
        .set_size(tauri::LogicalSize::new(expanded.width, height))
        .is_err()
    {
        state.compact_view.store(!compact, Ordering::Relaxed);
        return Err("Unable to resize the dashboard.".into());
    }
    if !compact {
        restore_window(&app)?;
    }
    Ok(())
}

fn saved_geometry_for_view(
    current: WindowGeometry,
    prior: Option<WindowGeometry>,
    compact: bool,
) -> WindowGeometry {
    if compact {
        if let Some(prior) = prior {
            return WindowGeometry {
                x: current.x,
                y: current.y,
                ..prior
            };
        }
    }
    current
}

#[derive(Clone, Copy)]
struct WorkArea {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
}

#[derive(Debug, PartialEq)]
struct RestoredGeometry {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

fn recover_geometry(saved: WindowGeometry, monitors: &[WorkArea]) -> Option<RestoredGeometry> {
    let valid: Vec<_> = monitors
        .iter()
        .filter(|m| m.width > 0 && m.height > 0 && m.scale.is_finite() && m.scale > 0.0)
        .collect();
    // Prefer the monitor containing the saved title bar, otherwise the primary.
    let monitor = valid
        .iter()
        .copied()
        .find(|m| {
            let x = i64::from(saved.x);
            let y = i64::from(saved.y);
            x >= i64::from(m.x)
                && x < i64::from(m.x) + i64::from(m.width)
                && y >= i64::from(m.y)
                && y < i64::from(m.y) + i64::from(m.height)
        })
        .or_else(|| valid.first().copied())?;
    let width = if saved.width.is_finite() {
        saved.width.clamp(320.0, 4096.0)
    } else {
        360.0
    };
    let height = if saved.height.is_finite() {
        saved.height.clamp(240.0, 4096.0)
    } else {
        480.0
    };
    // Leave room for native title bars/borders; positioning uses outer coordinates.
    let border = (16.0 * monitor.scale).ceil() as u32;
    let title = (48.0 * monitor.scale).ceil() as u32;
    let width =
        ((width * monitor.scale).round() as u32).min(monitor.width.saturating_sub(border).max(1));
    let height =
        ((height * monitor.scale).round() as u32).min(monitor.height.saturating_sub(title).max(1));
    let max_x = i64::from(monitor.x)
        + i64::from(monitor.width.saturating_sub(width.saturating_add(border)));
    let max_y = i64::from(monitor.y)
        + i64::from(monitor.height.saturating_sub(height.saturating_add(title)));
    Some(RestoredGeometry {
        x: i64::from(saved.x).clamp(i64::from(monitor.x), max_x) as i32,
        y: i64::from(saved.y).clamp(i64::from(monitor.y), max_y) as i32,
        width,
        height,
    })
}

/// Restore and recover position on startup and whenever the tray reveals Luma.
pub fn restore_window(app: &tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("Luma's window is unavailable.")?;
    let state = app.state::<PreferencesState>();
    let (preferences, geometry) = {
        let store = state
            .store
            .lock()
            .map_err(|_| "Unable to access local preferences.")?;
        (store.saved.preferences.clone(), store.saved.geometry)
    };
    window
        .set_always_on_top(preferences.pinned)
        .map_err(|_| "Unable to restore window pinning.")?;
    let Some(geometry) = geometry else {
        return Ok(());
    };
    let mut monitors = window
        .available_monitors()
        .map_err(|_| "Unable to inspect connected monitors.")?;
    if let Ok(Some(primary)) = window.primary_monitor() {
        monitors.sort_by_key(|monitor| monitor.position() != primary.position());
    }
    let areas: Vec<_> = monitors
        .iter()
        .map(|monitor| {
            let area = monitor.work_area();
            WorkArea {
                x: area.position.x,
                y: area.position.y,
                width: area.size.width,
                height: area.size.height,
                scale: monitor.scale_factor(),
            }
        })
        .collect();
    let target = if state.compact_view.load(Ordering::Relaxed) {
        WindowGeometry {
            height: 240.0,
            ..geometry
        }
    } else {
        geometry
    };
    if let Some(geometry) = recover_geometry(target, &areas) {
        window
            .set_size(PhysicalSize::new(geometry.width, geometry.height))
            .map_err(|_| "Unable to restore window size.")?;
        window
            .set_position(PhysicalPosition::new(geometry.x, geometry.y))
            .map_err(|_| "Unable to restore window position.")?;
    }
    Ok(())
}

pub fn persist_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if window.label() != "main" {
        return;
    }
    let Some(state) = window.app_handle().try_state::<PreferencesState>() else {
        return;
    };
    match event {
        tauri::WindowEvent::Moved(_)
        | tauri::WindowEvent::Resized(_)
        | tauri::WindowEvent::ScaleFactorChanged { .. } => {
            if window.is_minimized().unwrap_or(false) {
                return;
            }
            let (Ok(position), Ok(size), Ok(scale)) = (
                window.outer_position(),
                window.inner_size(),
                window.scale_factor(),
            ) else {
                return;
            };
            if size.width == 0 || size.height == 0 || scale <= 0.0 {
                return;
            }
            if let Ok(mut store) = state.store.lock() {
                let current = WindowGeometry {
                    x: position.x,
                    y: position.y,
                    width: f64::from(size.width) / scale,
                    height: f64::from(size.height) / scale,
                };
                store.saved.geometry = Some(saved_geometry_for_view(
                    current,
                    store.saved.geometry,
                    state.compact_view.load(Ordering::Relaxed),
                ));
            }
            let _ = state.geometry_changed.send(());
        }
        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Focused(false) => {
            if let Ok(store) = state.store.lock() {
                if let Err(message) = store.persist() {
                    let _ = window.emit("preferences-error", message);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_compact_view_preserves_expanded_dimensions() {
        let expanded = WindowGeometry {
            x: 10,
            y: 20,
            width: 500.0,
            height: 720.0,
        };
        let compact = WindowGeometry {
            x: 200,
            y: 100,
            width: 500.0,
            height: 240.0,
        };
        let saved = saved_geometry_for_view(compact, Some(expanded), true);
        assert_eq!(saved.width, 500.0);
        assert_eq!(saved.height, 720.0);
        assert_eq!((saved.x, saved.y), (200, 100));
        assert_eq!(
            saved_geometry_for_view(compact, Some(expanded), false),
            compact
        );
    }

    #[test]
    fn compact_recovery_uses_destination_dpi_and_expansion_fits_work_area() {
        let destination = WorkArea {
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
            scale: 1.0,
        };
        let compact = WindowGeometry {
            x: 900,
            y: 500,
            width: 360.0,
            height: 240.0,
        };
        let recovered = recover_geometry(compact, &[destination]).unwrap();
        assert_eq!(recovered.height, 240);
        assert_eq!(recovered.y, 500);
        let expanded = recover_geometry(
            WindowGeometry {
                height: 720.0,
                ..compact
            },
            &[destination],
        )
        .unwrap();
        assert!(expanded.y + expanded.height as i32 <= 800);
        let retina = recover_geometry(
            compact,
            &[WorkArea {
                width: 2560,
                height: 1600,
                scale: 2.0,
                ..destination
            }],
        )
        .unwrap();
        assert_eq!(retina.height, 480);
    }

    #[test]
    fn proxy_defaults_migrate_and_updates_persist_without_clobbering_other_settings() {
        let old: Preferences =
            serde_json::from_str(r#"{"refresh_interval_secs":900,"hide_titles":true}"#).unwrap();
        assert_eq!(old.proxy, crate::network::ProxySettings::default());
        let next = old
            .patched(&PreferencesPatch {
                proxy: Some(crate::network::ProxySettings {
                    mode: crate::network::ProxyMode::Http,
                    server: " http://127.0.0.1:7890 ".into(),
                }),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(next.proxy.server, "http://127.0.0.1:7890/");
        assert!(next.hide_titles);
        assert_eq!(next.refresh_interval_secs, 900);
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        store.saved.preferences = next.clone();
        store.persist().unwrap();
        let json: String = store
            .connection
            .query_row("SELECT value FROM preferences WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        let restored: SavedPreferences = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.preferences, next);
        assert!(next
            .patched(&PreferencesPatch {
                proxy: Some(crate::network::ProxySettings {
                    mode: crate::network::ProxyMode::Http,
                    server: "http://user:password@localhost:7890".into()
                }),
                ..Default::default()
            })
            .is_err());
    }

    #[test]
    fn validates_refresh_without_changing_other_preferences() {
        let original = Preferences::default();
        for interval in [0, 59, 3601, u32::MAX] {
            assert!(original
                .patched(&PreferencesPatch {
                    refresh_interval_secs: Some(interval),
                    ..Default::default()
                })
                .is_err());
        }
        for interval in [60, 300, 3600] {
            let next = original
                .patched(&PreferencesPatch {
                    refresh_interval_secs: Some(interval),
                    hide_titles: Some(true),
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(next.refresh_interval_secs, interval);
            assert!(next.hide_titles);
            assert!(next.pinned);
            assert!(!next.hide_paths);
        }
    }

    #[test]
    fn preferences_round_trip_without_secret_fields() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        store.saved.preferences.hide_paths = true;
        store.saved.preferences.refresh_interval_secs = 900;
        store.saved.geometry = Some(WindowGeometry {
            x: -400,
            y: 40,
            width: 360.0,
            height: 480.0,
        });
        store.persist().unwrap();
        let value: String = store
            .connection
            .query_row("SELECT value FROM preferences WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        let restored: SavedPreferences = serde_json::from_str(&value).unwrap();
        assert_eq!(restored.preferences, store.saved.preferences);
        assert_eq!(restored.geometry, store.saved.geometry);
    }

    #[test]
    fn reopens_persisted_preferences_and_rejects_future_schema() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "luma-preferences-{}-{nonce}.sqlite3",
            std::process::id()
        ));
        {
            let mut store = Store::open(&path).unwrap();
            store.saved.preferences.collapsed = true;
            store.saved.preferences.refresh_interval_secs = 120;
            store.persist().unwrap();
        }
        {
            let store = Store::open(&path).unwrap();
            assert!(store.saved.preferences.collapsed);
            assert_eq!(store.saved.preferences.refresh_interval_secs, 120);
            store
                .connection
                .execute_batch("PRAGMA user_version = 2")
                .unwrap();
        }
        assert!(Store::open(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn recovers_disconnected_monitor_and_preserves_negative_coordinates() {
        let primary = WorkArea {
            x: 0,
            y: 25,
            width: 1920,
            height: 1055,
            scale: 1.0,
        };
        let left = WorkArea {
            x: -1920,
            y: 25,
            ..primary
        };
        let saved = WindowGeometry {
            x: -1700,
            y: 70,
            width: 360.0,
            height: 480.0,
        };
        assert_eq!(recover_geometry(saved, &[primary, left]).unwrap().x, -1700);
        let recovered = recover_geometry(saved, &[primary]).unwrap();
        assert_eq!(recovered.x, 0);
        assert_eq!(recovered.y, 70);
    }

    #[test]
    fn dpi_and_work_area_changes_keep_title_bar_accessible() {
        let monitor = WorkArea {
            x: 0,
            y: 50,
            width: 1600,
            height: 900,
            scale: 2.0,
        };
        let saved = WindowGeometry {
            x: 1590,
            y: 895,
            width: 360.0,
            height: 480.0,
        };
        let result = recover_geometry(saved, &[monitor]).unwrap();
        assert_eq!(result.width, 720);
        assert_eq!(result.height, 804);
        assert!(result.x >= 0 && result.x + result.width as i32 <= 1600);
        assert_eq!(result.y, 50);
        assert!(recover_geometry(saved, &[]).is_none());
    }

    #[test]
    fn corrupt_dimensions_and_small_screens_are_bounded() {
        let monitor = WorkArea {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
            scale: 1.0,
        };
        let saved = WindowGeometry {
            x: i32::MAX,
            y: i32::MIN,
            width: f64::NAN,
            height: f64::INFINITY,
        };
        let result = recover_geometry(saved, &[monitor]).unwrap();
        assert_eq!(result.width, 360);
        assert_eq!(result.height, 480);
        assert_eq!(result.y, 0);
        assert_eq!(result.x, 424);
    }
}
