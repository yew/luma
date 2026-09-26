//! Bounded local history commands. Secrets are never accepted through this API.
use crate::storage::{
    AccountKey, HistoryPage, HistoryQuery, HistorySettings, Storage, StorageError,
};
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn history_settings(storage: State<'_, Arc<Storage>>) -> Result<HistorySettings, StorageError> {
    storage.settings()
}
#[tauri::command]
pub fn update_history_settings(
    storage: State<'_, Arc<Storage>>,
    enabled: bool,
    retention_days: u32,
) -> Result<HistorySettings, StorageError> {
    storage.set_settings(enabled, retention_days)
}
#[tauri::command]
pub fn query_usage_history(
    storage: State<'_, Arc<Storage>>,
    query: HistoryQuery,
) -> Result<HistoryPage, StorageError> {
    storage.query(query)
}
#[tauri::command]
pub fn clear_usage_history(
    storage: State<'_, Arc<Storage>>,
    account: Option<AccountKey>,
) -> Result<u64, StorageError> {
    storage.clear_history(account.as_ref())
}
#[tauri::command]
pub async fn clear_usage_cache(
    service: State<'_, crate::github::GitHubService>,
    app: tauri::AppHandle,
    account: Option<AccountKey>,
) -> Result<u64, StorageError> {
    service.clear_cached_snapshot(&app, account.as_ref()).await
}

#[tauri::command]
pub fn rebuild_usage_cache(
    storage: State<'_, Arc<Storage>>,
    account: AccountKey,
) -> Result<bool, StorageError> {
    storage.rebuild_cache(&account)
}
