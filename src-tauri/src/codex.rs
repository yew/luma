//! Native metadata-only Codex hook bridge. No private Codex IPC or transcript reads.
use crate::codex_state::{CodexStore, HookEvent, Session};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{Emitter, State};

static REVISION: AtomicU64 = AtomicU64::new(1);
const MODE: &str = "--luma-codex-hook-v1";
const LABEL: &str = "Luma session status";
const MAX_INPUT: u64 = 1024 * 1024;
const EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Stop",
    "Interrupt",
    "SessionEnd",
];
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn failure() -> String {
    "Unable to access Codex hook configuration. Check file access and retry setup.".into()
}

#[derive(Deserialize)]
struct HookInput {
    hook_event_name: String,
    session_id: String,
    turn_id: Option<String>,
    tool_name: Option<String>,
    tool_use_id: Option<String>,
    cwd: Option<String>,
    agent_id: Option<String>,
    agent_type: Option<String>,
}
fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}
fn normalize(bytes: &[u8], received_at: u64) -> Option<HookEvent> {
    let input: HookInput = serde_json::from_slice(bytes).ok()?;
    if !EVENTS.contains(&input.hook_event_name.as_str())
        || !valid_id(&input.session_id)
        || input.agent_id.as_ref().is_some_and(|v| !v.is_empty())
        || input.agent_type.as_ref().is_some_and(|v| !v.is_empty())
    {
        return None;
    }
    if !matches!(
        input.hook_event_name.as_str(),
        "SessionStart" | "SessionEnd"
    ) && !input.turn_id.as_deref().is_some_and(valid_id)
    {
        return None;
    }
    let project = input.cwd.and_then(|cwd| {
        // Only a basename is retained. Never keep an absolute project path.
        cwd.trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .next()
            .filter(|part| valid_id(part))
            .map(str::to_owned)
    });
    Some(HookEvent {
        event_id: uuid::Uuid::new_v4().to_string(),
        session_id: input.session_id,
        turn_id: input.turn_id.filter(|v| valid_id(v)),
        kind: input.hook_event_name,
        tool_name: input.tool_name.filter(|v| valid_id(v)),
        tool_use_id: input.tool_use_id.filter(|v| valid_id(v)),
        project,
        received_at,
    })
}

/// Run before Tauri initialization, so hooks never spawn a second dashboard.
pub fn run_hook_mode() -> bool {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_none_or(|arg| arg != MODE) {
        return false;
    }
    let received_at = now();
    // Bounded hook lifetime, including stalled stdin. Never block or approve Codex.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(1500));
        let _ = std::io::stdout().write_all(b"{}\n");
        std::process::exit(0);
    });
    if args.len() == 3 {
        let path = PathBuf::from(&args[2]);
        let mut bytes = Vec::new();
        if path.is_absolute()
            && path.is_file()
            && std::io::stdin()
                .take(MAX_INPUT + 1)
                .read_to_end(&mut bytes)
                .is_ok()
            && bytes.len() as u64 <= MAX_INPUT
        {
            if let Some(event) = normalize(&bytes, received_at) {
                if let Ok(store) = CodexStore::open(&path) {
                    let _ = store.append(event);
                }
            }
        }
    }
    let _ = std::io::stdout().write_all(b"{}\n");
    true
}

#[derive(Clone, Serialize, PartialEq)]
pub struct CodexStatus {
    pub revision: u64,
    pub enabled: bool,
    pub installed: bool,
    pub sessions: Vec<Session>,
    pub error: Option<String>,
}
pub struct CodexService {
    store: CodexStore,
    db_path: PathBuf,
    manifest: PathBuf,
    config_path: PathBuf,
    install_lock: Mutex<()>,
    snapshot_lock: Mutex<()>,
    reconciliation: Mutex<crate::codex_reconcile::Reconciler>,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    command: String,
    command_windows: String,
    #[serde(default)]
    prior_commands: Vec<(String, String)>,
}

impl CodexService {
    pub fn new(data: &Path, home: &Path) -> Result<Self, String> {
        let db_path = data.join("codex-hooks.sqlite3");
        let store = CodexStore::open(&db_path).map_err(|_| failure())?;
        store.mark_restarted(now()).map_err(|_| failure())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&db_path, fs::Permissions::from_mode(0o600))
                .map_err(|_| failure())?;
        }
        let codex_home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".codex"));
        Ok(Self {
            store,
            db_path,
            manifest: data.join("codex-hook-install.json"),
            config_path: codex_home.join("hooks.json"),
            install_lock: Mutex::new(()),
            snapshot_lock: Mutex::new(()),
            reconciliation: Mutex::new(crate::codex_reconcile::Reconciler::default()),
        })
    }
    fn config(&self) -> Result<Value, String> {
        read_config(&self.config_path)
    }
    fn previous(&self) -> Result<Option<Manifest>, String> {
        if !self.manifest.exists() {
            return Ok(None);
        }
        serde_json::from_value(read_config(&self.manifest)?)
            .map(Some)
            .map_err(|_| failure())
    }
    fn installed(&self) -> Result<bool, String> {
        let Some(previous) = self.previous()? else {
            return Ok(false);
        };
        let config = self.config()?;
        Ok(EVENTS.iter().all(|event| {
            config
                .get("hooks")
                .and_then(|v| v.get(event))
                .and_then(Value::as_array)
                .is_some_and(|groups| {
                    groups.iter().any(|group| {
                        group
                            .get("hooks")
                            .and_then(Value::as_array)
                            .is_some_and(|handlers| {
                                handlers.iter().any(|handler| {
                                    same_command(
                                        handler,
                                        &previous.command,
                                        &previous.command_windows,
                                    )
                                })
                            })
                    })
                })
        }))
    }
    pub fn reconcile_deleted(&self) {
        let Some(home) = self.config_path.parent() else {
            return;
        };
        let time = now();
        let Ok(sessions) = self.store.reconciliation_snapshot(time) else {
            return;
        };
        if sessions.is_empty() {
            return;
        }
        let Ok(mut reconciliation) = self.reconciliation.lock() else {
            return;
        };
        if let Some((missing, present)) = reconciliation.check(home, &sessions, time) {
            let _ = self.store.restore_present(&present);
            if !missing.is_empty() {
                let _ = self.store.hide_missing(&missing, time);
            }
        }
    }
    pub fn prune(&self) {
        let _ = self.store.prune_expired(now());
    }
    pub fn status(&self) -> CodexStatus {
        let _snapshot = self
            .snapshot_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let installed = self.installed();
        let enabled = self.store.enabled();
        let sessions = self.store.snapshot(now());
        let error = if installed.is_err() || enabled.is_err() || sessions.is_err() {
            Some(failure())
        } else {
            None
        };
        CodexStatus {
            revision: REVISION.fetch_add(1, Ordering::Relaxed),
            installed: installed.unwrap_or(false),
            enabled: enabled.unwrap_or(false),
            sessions: sessions.unwrap_or_default(),
            error,
        }
    }
    fn enable(&self) -> Result<CodexStatus, String> {
        let _local = self.install_lock.lock().map_err(|_| failure())?;
        let parent = self.config_path.parent().ok_or_else(failure)?;
        fs::create_dir_all(parent).map_err(|_| failure())?;
        let _guard = ConfigLock::acquire(parent)?;
        let config = self.config()?;
        let executable = std::env::current_exe().map_err(|_| failure())?;
        let mut manifest = make_manifest(&executable, &self.db_path)?;
        if let Some(previous) = self.previous()? {
            manifest.prior_commands = previous.prior_commands;
            let prior = (previous.command, previous.command_windows);
            if prior != (manifest.command.clone(), manifest.command_windows.clone())
                && !manifest.prior_commands.contains(&prior)
            {
                manifest.prior_commands.push(prior);
            }
            if manifest.prior_commands.len() > 64 {
                return Err(
                    "Remove old Luma hooks before changing the installation path again.".into(),
                );
            }
        }
        let next = merged(config.clone(), Some(&manifest), Some(&manifest))?;
        // A failed write never silently turns collection on.
        atomic_json(
            &self.manifest,
            &serde_json::to_value(&manifest).map_err(|_| failure())?,
        )?;
        if self.config()? != config {
            return Err(
                "Codex hooks changed during setup. Retry to preserve the latest configuration."
                    .into(),
            );
        }
        atomic_json(&self.config_path, &next)?;
        self.store.set_enabled(true).map_err(|_| failure())?;
        Ok(self.status())
    }
    fn disable(&self) -> Result<CodexStatus, String> {
        let _local = self.install_lock.lock().map_err(|_| failure())?;
        // Stop accepting events even if config removal fails.
        self.store.set_enabled(false).map_err(|_| failure())?;
        if let Some(previous) = self.previous()? {
            let parent = self.config_path.parent().ok_or_else(failure)?;
            let _guard = ConfigLock::acquire(parent)?;
            let config = self.config()?;
            let next = merged(config.clone(), Some(&previous), None)?;
            if self.config()? != config {
                return Err("Codex hooks changed during removal. Retry to preserve the latest configuration.".into());
            }
            atomic_json(&self.config_path, &next)?;
            fs::remove_file(&self.manifest).map_err(|_| failure())?;
        }
        Ok(self.status())
    }
}
fn quote_posix(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\"'\"'"))
}
fn encode_powershell(script: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        encoded.push(ALPHABET[((value >> 18) & 63) as usize] as char);
        encoded.push(ALPHABET[((value >> 12) & 63) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    encoded
}
fn make_manifest(executable: &Path, database: &Path) -> Result<Manifest, String> {
    let exe = executable.to_str().ok_or_else(failure)?;
    let db = database.to_str().ok_or_else(failure)?;
    let powershell = format!(
        "[Console]::In.ReadToEnd() | & '{}' {MODE} '{}'; exit 0",
        exe.replace('\'', "''"),
        db.replace('\'', "''")
    );
    Ok(Manifest {
        command: format!("{} {MODE} {}", quote_posix(exe), quote_posix(db)),
        prior_commands: Vec::new(),
        command_windows: format!(
            "powershell.exe -NoProfile -NonInteractive -EncodedCommand {}",
            encode_powershell(&powershell)
        ),
    })
}
fn same_command(handler: &Value, command: &str, windows: &str) -> bool {
    handler.get("type").and_then(Value::as_str) == Some("command")
        && handler.get("statusMessage").and_then(Value::as_str) == Some(LABEL)
        && handler.get("command").and_then(Value::as_str) == Some(command)
        && handler.get("commandWindows").and_then(Value::as_str) == Some(windows)
}
fn owned(handler: &Value, manifest: &Manifest) -> bool {
    same_command(handler, &manifest.command, &manifest.command_windows)
        || manifest
            .prior_commands
            .iter()
            .any(|(command, windows)| same_command(handler, command, windows))
}
fn merged(
    mut config: Value,
    previous: Option<&Manifest>,
    install: Option<&Manifest>,
) -> Result<Value, String> {
    let root = config.as_object_mut().ok_or_else(failure)?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(failure)?;
    for event in EVENTS {
        let groups = hooks
            .entry(*event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(failure)?;
        if let Some(previous) = previous {
            for group in groups.iter_mut() {
                let handlers = group
                    .get_mut("hooks")
                    .and_then(Value::as_array_mut)
                    .ok_or_else(failure)?;
                handlers.retain(|handler| !owned(handler, previous));
            }
            groups.retain(|group| {
                group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_none_or(|v| !v.is_empty())
            });
        }
        if let Some(manifest) = install {
            groups.push(json!({"hooks":[{"type":"command","command":manifest.command,"commandWindows":manifest.command_windows,"timeout":2,"async":false,"statusMessage":LABEL}]}));
        }
    }
    Ok(config)
}
fn read_config(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| failure())?;
    if metadata.file_type().is_symlink() || metadata.len() > MAX_INPUT {
        return Err(failure());
    }
    serde_json::from_slice(&fs::read(path).map_err(|_| failure())?).map_err(|_| failure())
}
fn atomic_json(path: &Path, value: &Value) -> Result<(), String> {
    let temp = path.with_extension(format!("luma-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temp).map_err(|_| failure())?;
        serde_json::to_writer_pretty(&mut file, value).map_err(|_| failure())?;
        file.write_all(b"\n").map_err(|_| failure())?;
        file.sync_all().map_err(|_| failure())?;
        fs::rename(&temp, path).map_err(|_| failure())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
// Advisory locking releases automatically if setup crashes. Never delete a lock
// inode while another process can hold it.
struct ConfigLock(fs::File);
impl ConfigLock {
    fn acquire(parent: &Path) -> Result<Self, String> {
        let path = parent.join(".luma-hooks.lock");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|_| failure())?;
        file.try_lock().map_err(|_| {
            "Codex hook setup is busy. Retry after the other setup finishes.".to_string()
        })?;
        Ok(Self(file))
    }
}
impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[tauri::command]
pub fn codex_status(service: State<'_, CodexService>) -> CodexStatus {
    service.status()
}
#[tauri::command]
pub async fn codex_enable(
    app: tauri::AppHandle,
    service: State<'_, CodexService>,
) -> Result<CodexStatus, String> {
    let status = service.enable()?;
    let _ = app.emit("codex-status", &status);
    Ok(status)
}
#[tauri::command]
pub async fn codex_disable(
    app: tauri::AppHandle,
    service: State<'_, CodexService>,
) -> Result<CodexStatus, String> {
    let result = service.disable();
    let _ = app.emit("codex-status", service.status());
    result
}
#[tauri::command]
pub async fn codex_clear_sessions(
    app: tauri::AppHandle,
    service: State<'_, CodexService>,
) -> Result<CodexStatus, String> {
    service.store.clear().map_err(|_| failure())?;
    let status = service.status();
    let _ = app.emit("codex-status", &status);
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_discards_prompts_outputs_paths_and_subagents() {
        let raw = br#"{"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"turn","cwd":"/private/project-name","prompt":"secret prompt","last_assistant_message":"secret response","tool_input":{"secret":"credential"},"transcript_path":"/private/transcript"}"#;
        let event = normalize(raw, 123).unwrap();
        assert_eq!(event.project.as_deref(), Some("project-name"));
        assert_eq!(event.received_at, 123);
        let mut sub: Value = serde_json::from_slice(raw).unwrap();
        sub["agent_id"] = json!("child");
        assert!(normalize(&serde_json::to_vec(&sub).unwrap(), 123).is_none());
        assert!(normalize(br#"{"hook_event_name":"Stop","session_id":"s"}"#, 0).is_none());
    }
    #[test]
    fn setup_and_disable_round_trip_preserve_user_handlers_on_disk() {
        let root = std::env::temp_dir().join(format!("luma-hook-setup-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let store = CodexStore::open(root.join("codex-hooks.sqlite3")).unwrap();
        let service = CodexService {
            store,
            db_path: root.join("codex-hooks.sqlite3"),
            manifest: root.join("manifest.json"),
            config_path: root.join("hooks.json"),
            install_lock: Mutex::new(()),
            snapshot_lock: Mutex::new(()),
            reconciliation: Mutex::new(crate::codex_reconcile::Reconciler::default()),
        };
        let original = json!({"description":"User preferences","hooks":{"Stop":[{"hooks":[{"type":"command","command":"user-handler"}]}]}});
        atomic_json(&service.config_path, &original).unwrap();
        let enabled = service.enable().unwrap();
        assert!(enabled.installed && enabled.enabled);
        let repeated = service.enable().unwrap();
        assert!(repeated.installed && repeated.enabled);
        assert_eq!(
            service.config().unwrap()["hooks"]["Stop"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let disabled = service.disable().unwrap();
        assert!(!disabled.installed && !disabled.enabled);
        assert_eq!(
            service.config().unwrap()["hooks"]["Stop"],
            original["hooks"]["Stop"]
        );
        assert_eq!(
            service.config().unwrap()["description"],
            original["description"]
        );
        drop(service);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn malformed_existing_config_is_not_overwritten_or_enabled() {
        let root = std::env::temp_dir().join(format!("luma-hook-bad-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let store = CodexStore::open(root.join("codex-hooks.sqlite3")).unwrap();
        let service = CodexService {
            store,
            db_path: root.join("codex-hooks.sqlite3"),
            manifest: root.join("manifest.json"),
            config_path: root.join("hooks.json"),
            install_lock: Mutex::new(()),
            snapshot_lock: Mutex::new(()),
            reconciliation: Mutex::new(crate::codex_reconcile::Reconciler::default()),
        };
        fs::write(&service.config_path, b"user's invalid json").unwrap();
        assert!(service.enable().is_err());
        assert!(!service.store.enabled().unwrap());
        assert_eq!(
            fs::read(&service.config_path).unwrap(),
            b"user's invalid json"
        );
        drop(service);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn advisory_setup_lock_is_reusable_after_owner_exits() {
        let root = std::env::temp_dir().join(format!("luma-hook-lock-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let first = ConfigLock::acquire(&root).unwrap();
        assert!(ConfigLock::acquire(&root).is_err());
        drop(first);
        assert!(ConfigLock::acquire(&root).is_ok());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn windows_encoding_preserves_literals_and_old_install_paths_are_removable() {
        assert_eq!(encode_powershell("A"), "QQA=");
        let old = make_manifest(Path::new("/old/app"), Path::new("/old/db")).unwrap();
        let mut replacement = make_manifest(Path::new("/new/app"), Path::new("/new/db")).unwrap();
        replacement
            .prior_commands
            .push((old.command.clone(), old.command_windows.clone()));
        let config = merged(json!({}), None, Some(&old)).unwrap();
        let repaired = merged(config, Some(&replacement), Some(&replacement)).unwrap();
        assert_eq!(repaired["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(
            repaired["hooks"]["Stop"][0]["hooks"][0]["command"],
            replacement.command
        );
    }

    #[test]
    fn hook_merge_is_idempotent_and_preserves_unrelated_configuration() {
        let manifest = make_manifest(
            Path::new("/Applications/Luma's App.app/luma"),
            Path::new("/tmp/data.sqlite"),
        )
        .unwrap();
        let existing = json!({"description":"User config","hooks":{"Stop":[{"hooks":[{"type":"command","command":"user-hook"}]}],"CustomEvent":[{"hooks":[]}]}});
        let once = merged(existing.clone(), None, Some(&manifest)).unwrap();
        let twice = merged(once.clone(), Some(&manifest), Some(&manifest)).unwrap();
        assert_eq!(once, twice);
        let removed = merged(twice, Some(&manifest), None).unwrap();
        assert_eq!(removed["hooks"]["Stop"], existing["hooks"]["Stop"]);
        assert_eq!(
            removed["hooks"]["CustomEvent"],
            existing["hooks"]["CustomEvent"]
        );
        assert_eq!(removed["description"], "User config");
        assert!(manifest.command.contains("'\"'\"'"));
        assert!(merged(json!({"hooks":[]}), None, Some(&manifest)).is_err());
    }
}
