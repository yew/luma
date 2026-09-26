//! Metadata-only Codex hook projection. Hooks are observed boundaries, never
//! proof of successful completion or a complete view of an agent process.
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    path::Path,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

const RETENTION_MS: u64 = 7 * 86_400_000;
const MAX_SESSIONS: usize = 200;
const MAX_EVENTS: usize = 20_000;
const MAX_WAITS: usize = 64;
const MAX_RETIRED_TURNS: usize = 64;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CodexError {
    pub code: &'static str,
    pub message: &'static str,
}
impl CodexError {
    fn database() -> Self {
        Self {
            code: "codex_storage",
            message: "Cannot access local Codex monitoring storage.",
        }
    }
    fn invalid() -> Self {
        Self {
            code: "invalid_codex_event",
            message: "Codex monitoring received invalid metadata.",
        }
    }
}
impl fmt::Display for CodexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}
impl std::error::Error for CodexError {}
impl From<rusqlite::Error> for CodexError {
    fn from(_: rusqlite::Error) -> Self {
        Self::database()
    }
}

/// The transport removes prompts, responses, tool arguments and hook bodies.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HookEvent {
    pub event_id: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub kind: String,
    pub tool_name: Option<String>,
    pub tool_use_id: Option<String>,
    pub project: Option<String>,
    pub received_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub session_id: String,
    pub turn_id: Option<String>,
    pub title: String,
    pub project: Option<String>,
    pub status: String,
    pub detail: String,
    pub last_activity: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PendingWait {
    tool_use_id: Option<String>,
    tool_name: Option<String>,
    input: bool,
    since: u64,
}
impl PendingWait {
    fn matches(&self, event: &HookEvent) -> bool {
        match (&self.tool_use_id, &event.tool_use_id) {
            (Some(expected), Some(actual)) => expected == actual,
            (Some(_), None) => false,
            (None, _) => self.tool_name.is_some() && self.tool_name == event.tool_name,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    session: Session,
    waits: Vec<PendingWait>,
    retired_turns: Vec<String>,
    turn_history_full: bool,
    waits_overflow: bool,
}

pub struct CodexStore {
    connection: Mutex<Connection>,
}
impl CodexStore {
    /// A dedicated database. Opening never changes session status because
    /// headless hook writers open it independently for each event.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CodexError> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_millis(500))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        Self::initialize(connection)
    }
    fn initialize(mut connection: Connection) -> Result<Self, CodexError> {
        connection.pragma_update(None, "secure_delete", "ON")?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > 1 {
            return Err(CodexError {
                code: "codex_schema_newer",
                message: "Codex monitoring storage needs a newer Luma version.",
            });
        }
        if version == 1 {
            return Ok(Self {
                connection: Mutex::new(connection),
            });
        }
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS codex_settings (
                id INTEGER PRIMARY KEY CHECK(id=1), enabled INTEGER NOT NULL CHECK(enabled IN (0,1))
             );
             INSERT OR IGNORE INTO codex_settings(id, enabled) VALUES(1, 0);
             CREATE TABLE IF NOT EXISTS codex_events (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT, event_id TEXT NOT NULL UNIQUE,
                received_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS codex_events_time ON codex_events(received_at);
             CREATE TABLE IF NOT EXISTS codex_sessions (
                session_id TEXT PRIMARY KEY, last_activity INTEGER NOT NULL, record TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS codex_sessions_time ON codex_sessions(last_activity);
             PRAGMA user_version=1;",
        )?;
        transaction.commit()?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
    fn lock(&self) -> Result<MutexGuard<'_, Connection>, CodexError> {
        self.connection.lock().map_err(|_| CodexError::database())
    }
    pub fn enabled(&self) -> Result<bool, CodexError> {
        Ok(self
            .lock()?
            .query_row("SELECT enabled FROM codex_settings WHERE id=1", [], |row| {
                row.get(0)
            })?)
    }
    pub fn set_enabled(&self, enabled: bool) -> Result<(), CodexError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("UPDATE codex_settings SET enabled=?1 WHERE id=1", [enabled])?;
        if !enabled {
            invalidate_active(
                &transaction,
                "Monitoring disabled; current status is unknown.",
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
    /// Called once by GUI setup, never by a headless writer. A restart does not
    /// advance last_activity because it is not new activity from the agent.
    pub fn mark_restarted(&self, now_ms: u64) -> Result<(), CodexError> {
        valid_time(now_ms)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        prune(&transaction, now_ms)?;
        invalidate_active(
            &transaction,
            "Luma restarted; waiting for a fresh runtime hook.",
        )?;
        transaction.commit()?;
        Ok(())
    }
    /// False means disabled, duplicate, or out-of-order; dedup and projection
    /// commit atomically across all writer processes.
    pub fn append(&self, event: HookEvent) -> Result<bool, CodexError> {
        validate(&event)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let enabled: bool =
            transaction.query_row("SELECT enabled FROM codex_settings WHERE id=1", [], |row| {
                row.get(0)
            })?;
        if !enabled {
            return Ok(false);
        }
        prune(&transaction, event.received_at)?;
        if transaction.execute(
            "INSERT OR IGNORE INTO codex_events(event_id, received_at) VALUES(?1, ?2)",
            params![event.event_id, event.received_at],
        )? == 0
        {
            transaction.commit()?;
            return Ok(false);
        }
        let existing: Option<String> = transaction
            .query_row(
                "SELECT record FROM codex_sessions WHERE session_id=?1",
                [&event.session_id],
                |row| row.get(0),
            )
            .optional()?;
        let mut record = match existing {
            Some(json) => {
                serde_json::from_str::<Record>(&json).map_err(|_| CodexError::database())?
            }
            None => new_record(&event),
        };
        let applied = project(&mut record, &event);
        if applied {
            save(&transaction, &record)?;
        }
        prune(&transaction, event.received_at)?;
        transaction.commit()?;
        Ok(applied)
    }
    /// A read-only view for frequent GUI polling. Expired rows are filtered
    /// here; physical cleanup happens on append, startup, and prune_expired.
    pub fn snapshot(&self, now_ms: u64) -> Result<Vec<Session>, CodexError> {
        valid_time(now_ms)?;
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT record FROM codex_sessions WHERE last_activity >= ?1
             ORDER BY last_activity DESC, session_id LIMIT ?2",
        )?;
        let jsons = statement.query_map(
            params![now_ms.saturating_sub(RETENTION_MS), MAX_SESSIONS],
            |row| row.get::<_, String>(0),
        )?;
        let mut sessions = Vec::new();
        for json in jsons {
            let record: Record =
                serde_json::from_str(&json?).map_err(|_| CodexError::database())?;
            sessions.push(record.session);
        }
        sessions.sort_by(|a, b| {
            priority(&a.status)
                .cmp(&priority(&b.status))
                .then_with(|| b.last_activity.cmp(&a.last_activity))
                .then_with(|| a.session_id.cmp(&b.session_id))
        });
        Ok(sessions)
    }
    /// Infrequent maintenance for a long-running GUI with no incoming hooks.
    pub fn prune_expired(&self, now_ms: u64) -> Result<(), CodexError> {
        valid_time(now_ms)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        prune(&transaction, now_ms)?;
        transaction.commit()?;
        Ok(())
    }
    /// Clear session metadata and dedup keys without changing the opt-in.
    pub fn clear(&self) -> Result<(), CodexError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch("DELETE FROM codex_sessions; DELETE FROM codex_events;")?;
        transaction.commit()?;
        Ok(())
    }
}
fn valid_time(time: u64) -> Result<(), CodexError> {
    if time > i64::MAX as u64 {
        Err(CodexError::invalid())
    } else {
        Ok(())
    }
}
fn valid_text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
fn validate(event: &HookEvent) -> Result<(), CodexError> {
    valid_time(event.received_at)?;
    if !valid_text(&event.event_id, 128)
        || !valid_text(&event.session_id, 128)
        || event.turn_id.as_ref().is_some_and(|s| !valid_text(s, 128))
        || event
            .tool_use_id
            .as_ref()
            .is_some_and(|s| !valid_text(s, 128))
        || event
            .tool_name
            .as_ref()
            .is_some_and(|s| !valid_text(s, 128))
        || event.project.as_ref().is_some_and(|s| !valid_text(s, 1024))
        || !matches!(
            event.kind.as_str(),
            "SessionStart"
                | "UserPromptSubmit"
                | "PreToolUse"
                | "PostToolUse"
                | "PermissionRequest"
                | "Stop"
                | "Interrupt"
                | "SessionEnd"
        )
    {
        return Err(CodexError::invalid());
    }
    Ok(())
}
fn priority(status: &str) -> u8 {
    match status {
        "waiting" => 0,
        "running" => 1,
        "stopped" | "canceled" => 2,
        _ => 3,
    }
}
fn new_record(event: &HookEvent) -> Record {
    Record {
        session: Session {
            session_id: event.session_id.clone(),
            turn_id: None,
            title: format!(
                "Codex session {}",
                event.session_id.chars().take(8).collect::<String>()
            ),
            project: event.project.clone(),
            status: "unknown".into(),
            detail: "Waiting for a runtime hook.".into(),
            last_activity: 0,
        },
        waits: vec![],
        retired_turns: vec![],
        turn_history_full: false,
        waits_overflow: false,
    }
}
fn set_state(record: &mut Record, status: &str, detail: &str) {
    record.session.status = status.into();
    record.session.detail = detail.into();
}
fn clear_waits(record: &mut Record) {
    record.waits.clear();
    record.waits_overflow = false;
}
fn wait_state(record: &mut Record) {
    if record.waits.iter().any(|wait| wait.input) {
        set_state(record, "waiting", "Input requested; response not observed.");
    } else {
        set_state(
            record,
            "waiting",
            "Approval requested; decision not observed.",
        );
    }
}
fn retire_turn(record: &mut Record) {
    if let Some(turn) = record.session.turn_id.take() {
        if record.retired_turns.len() == MAX_RETIRED_TURNS {
            record.retired_turns.remove(0);
            record.turn_history_full = true;
        }
        record.retired_turns.push(turn);
    }
}
fn input_tool(tool: Option<&str>) -> bool {
    matches!(
        tool,
        Some(
            "request_user_input"
                | "ask_user"
                | "functions.request_user_input"
                | "functions.ask_user"
        )
    )
}
fn project(record: &mut Record, event: &HookEvent) -> bool {
    if event.received_at < record.session.last_activity {
        return false;
    }
    if record.session.turn_id.is_some()
        && event.turn_id.is_none()
        && matches!(event.kind.as_str(), "Stop" | "Interrupt")
    {
        // An uncorrelated turn terminal cannot terminate a known newer turn.
        // SessionEnd is a session boundary and only makes the status unknown.
        return false;
    }
    if let Some(turn) = &event.turn_id {
        if record.retired_turns.contains(turn) {
            return false;
        }
        if record
            .session
            .turn_id
            .as_ref()
            .is_some_and(|current| current != turn)
        {
            let fresh_boundary = matches!(
                event.kind.as_str(),
                "UserPromptSubmit" | "PreToolUse" | "PermissionRequest"
            );
            if !fresh_boundary || (record.turn_history_full && event.kind != "UserPromptSubmit") {
                return false;
            }
            retire_turn(record);
            clear_waits(record);
        }
    }
    if event.kind == "UserPromptSubmit" && event.turn_id.is_none() {
        retire_turn(record);
    }
    if event.turn_id.is_some() {
        record.session.turn_id = event.turn_id.clone();
    }
    if event.project.is_some() {
        record.session.project = event.project.clone();
    }
    let tied_terminal = event.received_at == record.session.last_activity
        && matches!(record.session.status.as_str(), "running" | "waiting")
        && matches!(event.kind.as_str(), "Stop" | "Interrupt" | "SessionEnd");
    record.session.last_activity = event.received_at;
    if tied_terminal {
        clear_waits(record);
        set_state(
            record,
            "unknown",
            "Event order is uncertain; terminal hook was not applied.",
        );
        return true;
    }
    match event.kind.as_str() {
        "SessionStart" => {
            clear_waits(record);
            set_state(
                record,
                "unknown",
                "Session opened or resumed; waiting for a fresh runtime hook.",
            );
        }
        "SessionEnd" => {
            clear_waits(record);
            set_state(
                record,
                "unknown",
                "Session ended; turn outcome is not verified.",
            );
        }
        "UserPromptSubmit" => {
            clear_waits(record);
            set_state(
                record,
                "running",
                "Prompt submitted; runtime activity observed.",
            );
        }
        "PermissionRequest" | "PreToolUse" => {
            let input = input_tool(event.tool_name.as_deref());
            if event.kind == "PermissionRequest" || input {
                if !record.waits.iter().any(|wait| wait.matches(event)) {
                    if record.waits.len() < MAX_WAITS {
                        record.waits.push(PendingWait {
                            tool_use_id: event.tool_use_id.clone(),
                            tool_name: event.tool_name.clone(),
                            input,
                            since: event.received_at,
                        });
                    } else {
                        record.waits_overflow = true;
                    }
                }
                wait_state(record);
            } else if record.waits.is_empty() && !record.waits_overflow {
                set_state(
                    record,
                    "running",
                    "Tool started; runtime activity observed.",
                );
            } else {
                wait_state(record);
            }
        }
        "PostToolUse" => {
            // Same-millisecond callbacks cannot establish causal ordering.
            record
                .waits
                .retain(|wait| !wait.matches(event) || event.received_at <= wait.since);
            if record.waits.is_empty() && !record.waits_overflow {
                set_state(
                    record,
                    "running",
                    "Tool finished; turn outcome is not verified.",
                );
            } else {
                wait_state(record);
            }
        }
        "Stop" => {
            clear_waits(record);
            set_state(record, "stopped", "Turn stopped; completion not verified.");
        }
        "Interrupt" => {
            clear_waits(record);
            set_state(
                record,
                "canceled",
                "Turn interrupted; cancellation hook observed.",
            );
        }
        _ => return false,
    }
    true
}
fn save(transaction: &Transaction<'_>, record: &Record) -> Result<(), CodexError> {
    let json = serde_json::to_string(record).map_err(|_| CodexError::database())?;
    transaction.execute(
        "INSERT INTO codex_sessions(session_id,last_activity,record) VALUES(?1,?2,?3)
         ON CONFLICT(session_id) DO UPDATE SET last_activity=excluded.last_activity,record=excluded.record",
        params![record.session.session_id, record.session.last_activity, json])?;
    Ok(())
}
fn read_records(transaction: &Transaction<'_>) -> Result<Vec<Record>, CodexError> {
    let mut statement = transaction.prepare("SELECT record FROM codex_sessions")?;
    let jsons = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut records = vec![];
    for json in jsons {
        records.push(serde_json::from_str(&json?).map_err(|_| CodexError::database())?);
    }
    Ok(records)
}
fn invalidate_active(transaction: &Transaction<'_>, detail: &str) -> Result<(), CodexError> {
    for mut record in read_records(transaction)? {
        if matches!(record.session.status.as_str(), "running" | "waiting") {
            clear_waits(&mut record);
            set_state(&mut record, "unknown", detail);
            save(transaction, &record)?;
        }
    }
    Ok(())
}
fn prune(transaction: &Transaction<'_>, now_ms: u64) -> Result<(), CodexError> {
    let cutoff = now_ms.saturating_sub(RETENTION_MS);
    transaction.execute("DELETE FROM codex_events WHERE received_at < ?1", [cutoff])?;
    transaction.execute(
        "DELETE FROM codex_sessions WHERE last_activity < ?1",
        [cutoff],
    )?;
    transaction.execute("DELETE FROM codex_events WHERE sequence IN (
        SELECT sequence FROM codex_events ORDER BY received_at DESC, sequence DESC LIMIT -1 OFFSET ?1
    )", [MAX_EVENTS])?;
    transaction.execute("DELETE FROM codex_sessions WHERE session_id IN (
        SELECT session_id FROM codex_sessions ORDER BY last_activity DESC, session_id LIMIT -1 OFFSET ?1
    )", [MAX_SESSIONS])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store() -> CodexStore {
        let store = CodexStore::initialize(Connection::open_in_memory().unwrap()).unwrap();
        store.set_enabled(true).unwrap();
        store
    }
    fn event(kind: &str, time: u64) -> HookEvent {
        HookEvent {
            event_id: format!("{kind}-{time}"),
            session_id: "session-1".into(),
            turn_id: Some("turn-1".into()),
            kind: kind.into(),
            tool_name: None,
            tool_use_id: None,
            project: Some("luma".into()),
            received_at: time,
        }
    }
    fn tool(kind: &str, time: u64, name: &str, id: &str) -> HookEvent {
        HookEvent {
            tool_name: Some(name.into()),
            tool_use_id: Some(id.into()),
            ..event(kind, time)
        }
    }
    fn only(store: &CodexStore) -> Session {
        store.snapshot(1000).unwrap().remove(0)
    }

    #[test]
    fn opt_in_and_restart_do_not_fabricate_runtime_activity() {
        let store = CodexStore::initialize(Connection::open_in_memory().unwrap()).unwrap();
        assert!(!store.enabled().unwrap());
        assert!(!store.append(event("UserPromptSubmit", 10)).unwrap());
        assert!(store.snapshot(10).unwrap().is_empty());
        store.set_enabled(true).unwrap();
        store.append(event("UserPromptSubmit", 20)).unwrap();
        store.mark_restarted(30).unwrap();
        assert_eq!(only(&store).status, "unknown");
        assert_eq!(only(&store).last_activity, 20);
        store.append(event("PreToolUse", 40)).unwrap();
        assert_eq!(only(&store).status, "running");
        store.set_enabled(false).unwrap();
        assert_eq!(only(&store).status, "unknown");
        assert!(!store.append(event("Stop", 50)).unwrap());
    }
    #[test]
    fn stop_is_not_success_and_same_turn_may_resume() {
        let store = store();
        store.append(event("UserPromptSubmit", 10)).unwrap();
        store.append(event("Stop", 20)).unwrap();
        assert_eq!(only(&store).status, "stopped");
        assert!(only(&store).detail.contains("completion not verified"));
        store.append(event("PreToolUse", 30)).unwrap();
        assert_eq!(only(&store).status, "running");
        store.append(event("Interrupt", 40)).unwrap();
        assert_eq!(only(&store).status, "canceled");
        store.append(event("UserPromptSubmit", 50)).unwrap();
        assert_eq!(only(&store).status, "running");
        store.append(event("SessionEnd", 60)).unwrap();
        assert_eq!(only(&store).status, "unknown");
    }
    #[test]
    fn dedup_and_old_timestamps_cannot_overwrite_new_state() {
        let store = store();
        let prompt = event("UserPromptSubmit", 20);
        assert!(store.append(prompt.clone()).unwrap());
        assert!(!store.append(prompt).unwrap());
        assert!(!store.append(event("Stop", 10)).unwrap());
        assert_eq!(only(&store).status, "running");
        store.append(event("Stop", 20)).unwrap();
        assert_eq!(only(&store).status, "unknown");
    }
    #[test]
    fn late_events_from_retired_turns_do_not_overwrite_a_new_turn() {
        let store = store();
        store.append(event("UserPromptSubmit", 10)).unwrap();
        let mut next = event("UserPromptSubmit", 20);
        next.turn_id = Some("turn-2".into());
        store.append(next).unwrap();
        for kind in [
            "Stop",
            "PostToolUse",
            "PreToolUse",
            "Interrupt",
            "UserPromptSubmit",
        ] {
            assert!(!store.append(event(kind, 30)).unwrap());
        }
        assert_eq!(only(&store).turn_id.as_deref(), Some("turn-2"));
        assert_eq!(only(&store).status, "running");
        assert!(!store
            .append(HookEvent {
                turn_id: None,
                ..event("Stop", 35)
            })
            .unwrap());
        let mut unknown_old_stop = event("Stop", 40);
        unknown_old_stop.turn_id = Some("unseen-old-turn".into());
        assert!(!store.append(unknown_old_stop).unwrap());
        assert_eq!(only(&store).status, "running");
    }
    #[test]
    fn permission_wait_survives_unrelated_and_unidentified_callbacks() {
        let store = store();
        store
            .append(tool("PermissionRequest", 10, "shell", "a"))
            .unwrap();
        store
            .append(tool("PreToolUse", 20, "read_file", "b"))
            .unwrap();
        store.append(tool("PostToolUse", 30, "shell", "b")).unwrap();
        store
            .append(HookEvent {
                tool_name: Some("shell".into()),
                ..event("PostToolUse", 40)
            })
            .unwrap();
        assert_eq!(only(&store).status, "waiting");
        store.append(tool("PostToolUse", 50, "shell", "a")).unwrap();
        assert_eq!(only(&store).status, "running");
    }
    #[test]
    fn exact_input_tools_wait_and_matching_callback_resumes() {
        let store = store();
        store
            .append(tool("PreToolUse", 10, "request_user_input", "a"))
            .unwrap();
        assert!(only(&store).detail.starts_with("Input requested"));
        store
            .append(tool("PostToolUse", 10, "request_user_input", "a"))
            .unwrap();
        assert_eq!(only(&store).status, "waiting");
        store
            .append(tool("PostToolUse", 20, "request_user_input", "a"))
            .unwrap();
        assert_eq!(only(&store).status, "running");
        store
            .append(tool("PreToolUse", 30, "my_request_user_input_helper", "b"))
            .unwrap();
        assert_eq!(only(&store).status, "running");
    }
    #[test]
    fn fallback_matching_and_multiple_pending_waits_are_conservative() {
        let store = store();
        store
            .append(HookEvent {
                tool_name: Some("shell".into()),
                ..event("PermissionRequest", 10)
            })
            .unwrap();
        store
            .append(tool("PreToolUse", 20, "ask_user", "input"))
            .unwrap();
        store
            .append(tool("PostToolUse", 30, "shell", "shell-id"))
            .unwrap();
        assert_eq!(only(&store).status, "waiting");
        store
            .append(tool("PostToolUse", 40, "ask_user", "input"))
            .unwrap();
        assert_eq!(only(&store).status, "running");
        store.append(event("PermissionRequest", 50)).unwrap();
        store.append(event("PostToolUse", 60)).unwrap();
        assert_eq!(only(&store).status, "waiting");
        store.append(event("UserPromptSubmit", 70)).unwrap();
        assert_eq!(only(&store).status, "running");
    }
    #[test]
    fn resume_is_unknown_and_ordering_prioritizes_waiting() {
        let store = store();
        store.append(event("SessionStart", 10)).unwrap();
        assert_eq!(only(&store).status, "unknown");
        for (kind, session_id, time) in [
            ("Stop", "stopped", 20),
            ("UserPromptSubmit", "running", 30),
            ("PermissionRequest", "waiting", 15),
        ] {
            store
                .append(HookEvent {
                    session_id: session_id.into(),
                    ..event(kind, time)
                })
                .unwrap();
        }
        let statuses: Vec<_> = store
            .snapshot(100)
            .unwrap()
            .into_iter()
            .map(|s| s.status)
            .collect();
        assert_eq!(statuses, ["waiting", "running", "stopped", "unknown"]);
    }
    #[test]
    fn retention_caps_metadata_and_clear_preserves_opt_in() {
        let store = store();
        for number in 1..=MAX_SESSIONS + 2 {
            store
                .append(HookEvent {
                    session_id: format!("s-{number}"),
                    ..event("UserPromptSubmit", number as u64)
                })
                .unwrap();
        }
        assert_eq!(store.snapshot(1000).unwrap().len(), MAX_SESSIONS);
        assert!(store.snapshot(RETENTION_MS + 1000).unwrap().is_empty());
        store.prune_expired(RETENTION_MS + 1000).unwrap();
        let conn = store.lock().unwrap();
        let events: u64 = conn
            .query_row("SELECT COUNT(*) FROM codex_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, 0);
        drop(conn);
        store.append(event("Stop", RETENTION_MS + 2000)).unwrap();
        store.clear().unwrap();
        assert!(store.enabled().unwrap());
        assert!(store.snapshot(RETENTION_MS + 2000).unwrap().is_empty());
    }
    #[test]
    fn uncorrelated_session_end_clears_waiting_without_claiming_completion() {
        let store = store();
        store.append(event("PermissionRequest", 10)).unwrap();
        store
            .append(HookEvent {
                turn_id: None,
                ..event("SessionEnd", 20)
            })
            .unwrap();
        assert_eq!(only(&store).status, "unknown");
        assert_eq!(
            only(&store).detail,
            "Session ended; turn outcome is not verified."
        );
    }
    #[test]
    fn snapshots_do_not_write_or_wait_for_another_writer() {
        let path =
            std::env::temp_dir().join(format!("luma-codex-read-{}.sqlite", uuid::Uuid::new_v4()));
        {
            let reader = CodexStore::open(&path).unwrap();
            reader.set_enabled(true).unwrap();
            reader.append(event("UserPromptSubmit", 10)).unwrap();
            let mut other_connection = Connection::open(&path).unwrap();
            let transaction = other_connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            transaction
                .execute("UPDATE codex_settings SET enabled=0 WHERE id=1", [])
                .unwrap();
            // WAL readers see the committed snapshot despite an active writer.
            assert_eq!(only(&reader).status, "running");
            assert!(reader.snapshot(RETENTION_MS + 100).unwrap().is_empty());
            transaction.rollback().unwrap();
            let count: u64 = other_connection
                .query_row("SELECT COUNT(*) FROM codex_sessions", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 1);
        }
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn invalid_metadata_errors_never_echo_private_content() {
        let store = store();
        let secret = "private body
should never appear";
        let error = store
            .append(HookEvent {
                session_id: secret.into(),
                ..event("Stop", 10)
            })
            .unwrap_err();
        assert!(!error.to_string().contains(secret));
        assert!(store.append(event("Notification", 10)).is_err());
        assert!(store.append(event("Stop", u64::MAX)).is_err());
        assert!(store.snapshot(100).unwrap().is_empty());
    }
    #[test]
    fn separate_connections_preserve_settings_and_do_not_restart_sessions() {
        let path = std::env::temp_dir().join(format!("luma-codex-{}.sqlite", uuid::Uuid::new_v4()));
        {
            let writer = CodexStore::open(&path).unwrap();
            writer.set_enabled(true).unwrap();
            writer.append(event("UserPromptSubmit", 10)).unwrap();
            let reader = CodexStore::open(&path).unwrap();
            assert!(reader.enabled().unwrap());
            assert_eq!(only(&reader).status, "running");
            reader.mark_restarted(20).unwrap();
            assert_eq!(only(&writer).status, "unknown");
            writer.append(event("PreToolUse", 30)).unwrap();
            assert_eq!(only(&reader).status, "running");
        }
        let _ = std::fs::remove_file(path);
    }
}
