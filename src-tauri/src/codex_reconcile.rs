//! Existence checks only: the known Codex index and rollout filenames are never
//! used to infer runtime state, and conversation contents are never opened.
use crate::codex_state::Session;
use rusqlite::{Connection, OpenFlags};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    time::{Duration, Instant},
};
const GRACE_MS: u64 = 30_000;
const CONFIRM_MS: u64 = 5_000;

#[derive(Default)]
pub struct Reconciler {
    missing: HashMap<String, (u64, u64)>,
}
impl Reconciler {
    pub fn check(
        &mut self,
        home: &Path,
        sessions: &[Session],
        now: u64,
    ) -> Option<(Vec<Session>, HashSet<String>)> {
        let presence = source_presence(home, sessions);
        let Ok(present) = presence else {
            self.missing.clear();
            return None;
        };
        let mut deleted = vec![];
        self.missing
            .retain(|id, _| sessions.iter().any(|s| &s.session_id == id));
        for session in sessions {
            if present.contains(&session.session_id)
                || now.saturating_sub(session.last_activity) < GRACE_MS
            {
                self.missing.remove(&session.session_id);
                continue;
            }
            let entry = self
                .missing
                .entry(session.session_id.clone())
                .or_insert((now, session.last_activity));
            if entry.1 != session.last_activity {
                *entry = (now, session.last_activity);
            }
            if now.saturating_sub(entry.0) >= CONFIRM_MS {
                deleted.push(session.clone());
            }
        }
        Some((deleted, present))
    }
}
fn source_presence(home: &Path, sessions: &[Session]) -> Result<HashSet<String>, ()> {
    let index = home.join("state_5.sqlite");
    // A newer/different metadata layout is unavailable, never proof of deletion.
    for entry in fs::read_dir(home).map_err(|_| ())? {
        let entry = entry.map_err(|_| ())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("state_") && name.ends_with(".sqlite") && name != "state_5.sqlite" {
            return Err(());
        }
    }
    if !fs::symlink_metadata(&index).map_err(|_| ())?.is_file() {
        return Err(());
    }
    let mut con = Connection::open_with_flags(
        &index,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| ())?;
    con.busy_timeout(Duration::from_millis(100))
        .map_err(|_| ())?;
    con.pragma_update(None, "query_only", true)
        .map_err(|_| ())?;
    let columns: HashSet<String> = con
        .prepare("PRAGMA table_info(threads)")
        .map_err(|_| ())?
        .query_map([], |row| row.get(1))
        .map_err(|_| ())?
        .collect::<Result<_, _>>()
        .map_err(|_| ())?;
    if !["id", "archived", "rollout_path"]
        .iter()
        .all(|name| columns.contains(*name))
    {
        return Err(());
    }
    let tx = con.transaction().map_err(|_| ())?;
    let mut present = HashSet::new();
    let mut query = tx
        .prepare("SELECT EXISTS(SELECT 1 FROM threads WHERE id=?1)")
        .map_err(|_| ())?;
    let mut unresolved = HashSet::new();
    for session in sessions {
        // Only UUID-backed Codex threads are eligible. Synthetic/unknown IDs are
        // retained instead of interpreting another source's ID as a deletion.
        if uuid::Uuid::parse_str(&session.session_id).is_err() {
            present.insert(session.session_id.clone());
            continue;
        }
        let exists: bool = query
            .query_row([&session.session_id], |row| row.get(0))
            .map_err(|_| ())?;
        if exists {
            present.insert(session.session_id.clone());
        } else {
            unresolved.insert(session.session_id.clone());
        }
    }
    if unresolved.is_empty() {
        return Ok(present);
    }
    // Missing rows alone are not enough: historical/archived rollout files can
    // exist before index backfill. Check filenames, never file contents.
    let active = home.join("sessions");
    if !active.is_dir() {
        return Err(());
    }
    let mut budget = 50_000;
    let deadline = Instant::now() + Duration::from_millis(200);
    scan_filenames(&active, 0, &mut budget, &unresolved, &mut present, deadline)?;
    let archive = home.join("archived_sessions");
    match fs::symlink_metadata(&archive) {
        Ok(_) => scan_filenames(
            &archive,
            0,
            &mut budget,
            &unresolved,
            &mut present,
            deadline,
        )?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(()),
    }
    Ok(present)
}
fn scan_filenames(
    path: &Path,
    depth: usize,
    budget: &mut usize,
    unresolved: &HashSet<String>,
    present: &mut HashSet<String>,
    deadline: Instant,
) -> Result<(), ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() || depth > 5 {
        return Err(());
    }
    for entry in fs::read_dir(path).map_err(|_| ())? {
        if *budget == 0 || Instant::now() >= deadline {
            return Err(());
        }
        *budget -= 1;
        let entry = entry.map_err(|_| ())?;
        let kind = entry.file_type().map_err(|_| ())?;
        if kind.is_symlink() {
            return Err(());
        }
        if kind.is_dir() {
            scan_filenames(
                &entry.path(),
                depth + 1,
                budget,
                unresolved,
                present,
                deadline,
            )?;
        } else {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            for id in unresolved {
                if name.starts_with("rollout-") && name.ends_with(&format!("-{id}.jsonl")) {
                    present.insert(id.clone());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        root: std::path::PathBuf,
        db: Connection,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("luma-delete-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(root.join("sessions")).unwrap();
            let db = Connection::open(root.join("state_5.sqlite")).unwrap();
            db.execute_batch(
                "CREATE TABLE threads(id TEXT PRIMARY KEY, archived INTEGER, rollout_path TEXT)",
            )
            .unwrap();
            Self { root, db }
        }
        fn add(&self, id: &str, archived: bool) {
            self.db
                .execute(
                    "INSERT INTO threads VALUES(?1,?2,'not-read')",
                    rusqlite::params![id, archived],
                )
                .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn session() -> Session {
        Session {
            session_id: uuid::Uuid::new_v4().to_string(),
            turn_id: None,
            title: "Test".into(),
            project: None,
            status: "stopped".into(),
            detail: "".into(),
            last_activity: 1_000,
        }
    }
    fn check(
        r: &mut Reconciler,
        home: &Path,
        sessions: &[Session],
        now: u64,
    ) -> Vec<(String, u64)> {
        r.check(home, sessions, now)
            .map(|(missing, _)| {
                missing
                    .into_iter()
                    .map(|s| (s.session_id, s.last_activity))
                    .collect()
            })
            .unwrap_or_default()
    }
    #[test]
    fn source_deletion_removes_visible_card_and_late_stop_cannot_restore_it() {
        use crate::codex_state::{CodexStore, HookEvent};
        let f = Fixture::new();
        let id = uuid::Uuid::new_v4().to_string();
        let local = CodexStore::open(f.root.join("luma.sqlite3")).unwrap();
        local.set_enabled(true).unwrap();
        local
            .append(HookEvent {
                event_id: "first".into(),
                session_id: id.clone(),
                turn_id: Some("turn".into()),
                kind: "UserPromptSubmit".into(),
                tool_name: None,
                tool_use_id: None,
                project: None,
                received_at: 1000,
            })
            .unwrap();
        f.add(&id, false);
        let mut r = Reconciler::default();
        assert!(check(
            &mut r,
            &f.root,
            &local.reconciliation_snapshot(40000).unwrap(),
            40000
        )
        .is_empty());
        f.db.execute("DELETE FROM threads WHERE id=?1", [&id])
            .unwrap();
        assert!(check(
            &mut r,
            &f.root,
            &local.reconciliation_snapshot(45000).unwrap(),
            45000
        )
        .is_empty());
        let (missing, present) = r
            .check(
                &f.root,
                &local.reconciliation_snapshot(50000).unwrap(),
                50000,
            )
            .unwrap();
        local.restore_present(&present).unwrap();
        local.hide_missing(&missing, 50000).unwrap();
        assert!(local.snapshot(50000).unwrap().is_empty());
        assert!(!local
            .append(HookEvent {
                event_id: "late".into(),
                session_id: id.clone(),
                turn_id: Some("turn".into()),
                kind: "Stop".into(),
                tool_name: None,
                tool_use_id: None,
                project: None,
                received_at: 51000
            })
            .unwrap());
        f.add(&id, true);
        let (_, present) = r
            .check(
                &f.root,
                &local.reconciliation_snapshot(55000).unwrap(),
                55000,
            )
            .unwrap();
        local.restore_present(&present).unwrap();
        assert_eq!(local.snapshot(55000).unwrap()[0].status, "unknown");
    }
    #[test]
    fn removes_only_confirmed_deleted_threads_and_preserves_archives() {
        let f = Fixture::new();
        let active = session();
        let archive = session();
        let deleted = session();
        f.add(&active.session_id, false);
        f.add(&archive.session_id, true);
        f.add(&deleted.session_id, false);
        let sessions = vec![active, archive, deleted.clone()];
        let mut r = Reconciler::default();
        assert!(check(&mut r, &f.root, &sessions, 40_000).is_empty());
        f.db.execute("DELETE FROM threads WHERE id=?1", [&deleted.session_id])
            .unwrap();
        assert!(check(&mut r, &f.root, &sessions, 45_000).is_empty());
        assert!(check(&mut r, &f.root, &sessions, 49_999).is_empty());
        assert_eq!(
            check(&mut r, &f.root, &sessions, 50_000),
            vec![(deleted.session_id, 1000)]
        );
    }
    #[test]
    fn absent_or_invalid_index_never_deletes_and_resets_confirmation() {
        let f = Fixture::new();
        let sessions = vec![session()];
        let mut r = Reconciler::default();
        assert!(check(&mut r, &f.root, &sessions, 40_000).is_empty());
        f.db.execute_batch("ALTER TABLE threads RENAME TO old_threads;")
            .unwrap();
        assert!(check(&mut r, &f.root, &sessions, 50_000).is_empty());
        f.db.execute_batch("ALTER TABLE old_threads RENAME TO threads;")
            .unwrap();
        assert!(check(&mut r, &f.root, &sessions, 60_000).is_empty());
        fs::write(f.root.join("state_6.sqlite"), b"unsupported").unwrap();
        assert!(check(&mut r, &f.root, &sessions, 70_000).is_empty());
        assert!(check(&mut r, &f.root.join("missing"), &sessions, 80_000).is_empty());
    }
    #[test]
    fn fresh_hooks_and_backfilled_or_archived_rollouts_are_retained() {
        let f = Fixture::new();
        let fresh = session();
        let archived = session();
        let backfill = session();
        fs::create_dir_all(f.root.join("archived_sessions")).unwrap();
        fs::write(
            f.root
                .join("archived_sessions")
                .join(format!("rollout-date-{}.jsonl", archived.session_id)),
            b"DO NOT READ THIS",
        )
        .unwrap();
        fs::write(
            f.root
                .join("sessions")
                .join(format!("rollout-date-{}.jsonl", backfill.session_id)),
            b"DO NOT READ THIS",
        )
        .unwrap();
        let sessions = vec![fresh.clone(), archived, backfill];
        let mut r = Reconciler::default();
        assert!(check(&mut r, &f.root, &sessions, 5_000).is_empty());
        assert!(check(&mut r, &f.root, &sessions, 10_000).is_empty());
        assert!(check(&mut r, &f.root, &sessions, 40_000).is_empty());
        assert_eq!(
            check(&mut r, &f.root, &sessions, 45_000),
            vec![(fresh.session_id, 1000)]
        );
    }
}
