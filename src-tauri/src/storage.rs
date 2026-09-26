//! Local usage persistence. Quantities use exact decimal text; secrets and
//! conversation content must never be supplied to this module.
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use uuid::Uuid;

const DAY_MS: u64 = 86_400_000;
const MAX_GAP_MS: u64 = 15 * 60 * 1000;
const SCHEMA_VERSION: i64 = 2;
const MAX_PROJECTION_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct StorageError {
    pub code: &'static str,
    pub message: &'static str,
}
impl StorageError {
    fn database() -> Self {
        Self {
            code: "usage_storage",
            message: "Cannot access local usage storage.",
        }
    }
    fn invalid() -> Self {
        Self {
            code: "invalid_usage_data",
            message: "Usage storage received invalid data.",
        }
    }
}
impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}
impl std::error::Error for StorageError {}
impl From<rusqlite::Error> for StorageError {
    fn from(_: rusqlite::Error) -> Self {
        Self::database()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountKey {
    pub provider: String,
    pub host: String,
    /// A stable provider ID. A login fallback must be prefixed with login:.
    pub account_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistorySettings {
    pub enabled: bool,
    pub retention_days: u32,
}

/// A validated metric from one fresh provider response, never a cached response.
/// Unknown values remain None. Provider decimal spellings are retained.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObservationInput {
    pub metric: String,
    pub unit: String,
    pub semantics_version: String,
    pub aggregation_kind: String,
    pub semantics_verified: bool,
    pub fetched_at: u64,
    /// Only set when the provider timestamp's meaning has been verified.
    pub observed_at: Option<u64>,
    pub plan: Option<String>,
    pub entitlement: Option<String>,
    pub allowance_kind: String,
    pub used: Option<String>,
    pub remaining: Option<String>,
    pub reported_percent_remaining: Option<String>,
    pub overage_permitted: Option<bool>,
    pub period_id: Option<String>,
    pub period_start: Option<String>,
    pub period_end: Option<String>,
    pub reset_at: Option<String>,
    pub source_version: String,
    pub quality_flags: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageObservation {
    pub observation_id: i64,
    pub collection_id: String,
    pub series_id: i64,
    pub account: AccountKey,
    #[serde(flatten)]
    pub metric: ObservationInput,
    pub segment_id: String,
    pub boundary_reason: Option<String>,
    /// Unknown always forbids consumption deltas. Confirmed continuity still
    /// requires two observations within the same segment and compatible series.
    pub continuity_confidence: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryCursor {
    pub fetched_at: u64,
    pub observation_id: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryQuery {
    pub account: AccountKey,
    pub metric: String,
    pub unit: String,
    pub semantics_version: String,
    pub from: u64,
    /// Exclusive UTC millisecond upper bound.
    pub to: u64,
    pub limit: u32,
    pub cursor: Option<HistoryCursor>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryPage {
    pub observations: Vec<UsageObservation>,
    pub next_cursor: Option<HistoryCursor>,
}

pub struct Storage {
    connection: Mutex<Connection>,
}
impl Storage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        Self::initialize(connection)
    }
    #[cfg(test)]
    pub fn in_memory() -> Result<Self, StorageError> {
        Self::initialize(Connection::open_in_memory()?)
    }
    fn initialize(mut connection: Connection) -> Result<Self, StorageError> {
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "secure_delete", "ON")?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(StorageError {
                code: "usage_schema_newer",
                message: "This usage database needs a newer Luma version.",
            });
        }
        let transaction = connection.transaction()?;
        if version < 1 {
            transaction.execute_batch(MIGRATION_1)?;
        }
        if version < 2 {
            transaction.execute_batch(MIGRATION_2)?;
        }
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        transaction.commit()?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
    fn lock(&self) -> Result<MutexGuard<'_, Connection>, StorageError> {
        self.connection.lock().map_err(|_| StorageError::database())
    }
    pub fn settings(&self) -> Result<HistorySettings, StorageError> {
        let connection = self.lock()?;
        read_settings(&connection)
    }
    pub fn set_settings(
        &self,
        enabled: bool,
        retention_days: u32,
    ) -> Result<HistorySettings, StorageError> {
        if !(1..=3650).contains(&retention_days) {
            return Err(StorageError::invalid());
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as u64;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE usage_settings SET enabled=?1,retention_days=?2 WHERE singleton=1",
            params![enabled, retention_days],
        )?;
        prune_transaction(&transaction, now, retention_days)?;
        transaction.commit()?;
        Ok(HistorySettings {
            enabled,
            retention_days,
        })
    }

    /// Atomically appends all validated metrics and updates latest. Returns false
    /// for a replay. Older collections enter history but never replace newer
    /// cache. Replay IDs are retained for the same duration as history.
    pub fn record_success(
        &self,
        collection_id: &str,
        account: &AccountKey,
        started_at: u64,
        finished_at: u64,
        observations: &[ObservationInput],
        projection: &Value,
    ) -> Result<bool, StorageError> {
        validate_collection(collection_id, account, started_at, finished_at)?;
        if observations.len() > 64 {
            return Err(StorageError::invalid());
        }
        validate_projection(projection)?;
        let projection_json =
            serde_json::to_string(projection).map_err(|_| StorageError::invalid())?;
        if projection_json.len() > MAX_PROJECTION_BYTES {
            return Err(StorageError::invalid());
        }
        let mut identities = HashSet::new();
        for observation in observations {
            validate_observation(observation)?;
            if observation.fetched_at < started_at
                || observation.fetched_at > finished_at
                || !identities.insert((
                    &observation.metric,
                    &observation.unit,
                    &observation.semantics_version,
                ))
            {
                return Err(StorageError::invalid());
            }
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        if is_replay(&transaction, collection_id, account)? {
            return Ok(false);
        }
        let settings = read_settings(&transaction)?;
        if settings.enabled {
            transaction.execute("INSERT INTO usage_collections(collection_id,provider,host,account_id,started_at,finished_at,status,error_category,projection_json) VALUES(?1,?2,?3,?4,?5,?6,'success',NULL,?7)",
                params![collection_id, account.provider, account.host, account.account_id, started_at as i64, finished_at as i64, projection_json])?;
            for observation in observations {
                insert_observation(&transaction, collection_id, account, observation)?;
            }
        }
        transaction.execute("INSERT INTO usage_latest(provider,host,account_id,collection_id,fetched_at,projection_json) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(provider,host,account_id) DO UPDATE SET collection_id=excluded.collection_id,fetched_at=excluded.fetched_at,projection_json=excluded.projection_json WHERE excluded.fetched_at>usage_latest.fetched_at OR (excluded.fetched_at=usage_latest.fetched_at AND excluded.collection_id>usage_latest.collection_id)",
            params![account.provider, account.host, account.account_id, collection_id, finished_at as i64, projection_json])?;
        prune_transaction(&transaction, finished_at, settings.retention_days)?;
        transaction.commit()?;
        Ok(true)
    }

    /// Records a sanitized category, never a provider body or error message.
    /// Disabled history also disables failure-outcome writes.
    pub fn record_failure(
        &self,
        collection_id: &str,
        account: &AccountKey,
        started_at: u64,
        finished_at: u64,
        error_category: &str,
    ) -> Result<bool, StorageError> {
        validate_collection(collection_id, account, started_at, finished_at)?;
        if !matches!(
            error_category,
            "network"
                | "rate_limited"
                | "reauth_required"
                | "permission_denied"
                | "endpoint_unavailable"
                | "schema"
                | "http"
                | "cancelled"
                | "identity_mismatch"
                | "metric_unavailable"
                | "unknown"
        ) {
            return Err(StorageError::invalid());
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        if is_replay(&transaction, collection_id, account)? {
            return Ok(false);
        }
        let settings = read_settings(&transaction)?;
        if !settings.enabled {
            return Ok(false);
        }
        transaction.execute("INSERT INTO usage_collections(collection_id,provider,host,account_id,started_at,finished_at,status,error_category) VALUES(?1,?2,?3,?4,?5,?6,'failure',?7)",
            params![collection_id, account.provider, account.host, account.account_id, started_at as i64, finished_at as i64, error_category])?;
        prune_transaction(&transaction, finished_at, settings.retention_days)?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn latest(&self, account: &AccountKey) -> Result<Option<Value>, StorageError> {
        validate_account(account)?;
        let raw: Option<String> = self.lock()?.query_row("SELECT projection_json FROM usage_latest WHERE provider=?1 AND host=?2 AND account_id=?3",
            params![account.provider, account.host, account.account_id], |row| row.get(0)).optional()?;
        raw.map(|json| serde_json::from_str(&json).map_err(|_| StorageError::database()))
            .transpose()
    }

    /// Explicitly rebuilds this cache from retained successful collections.
    /// Merely reading a cleared cache never silently repopulates it.
    pub fn rebuild_cache(&self, account: &AccountKey) -> Result<bool, StorageError> {
        validate_account(account)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let updated = transaction.execute("INSERT INTO usage_latest(provider,host,account_id,collection_id,fetched_at,projection_json) SELECT provider,host,account_id,collection_id,finished_at,projection_json FROM usage_collections WHERE provider=?1 AND host=?2 AND account_id=?3 AND status='success' ORDER BY finished_at DESC,collection_id DESC LIMIT 1 ON CONFLICT(provider,host,account_id) DO UPDATE SET collection_id=excluded.collection_id,fetched_at=excluded.fetched_at,projection_json=excluded.projection_json WHERE excluded.fetched_at>=usage_latest.fetched_at",
            params![account.provider, account.host, account.account_id])?;
        transaction.commit()?;
        Ok(updated > 0)
    }

    pub fn query(&self, query: HistoryQuery) -> Result<HistoryPage, StorageError> {
        validate_account(&query.account)?;
        if query.from > query.to
            || query.to > i64::MAX as u64
            || !(1..=500).contains(&query.limit)
            || !valid_text(&query.metric)
            || !valid_text(&query.unit)
            || !valid_text(&query.semantics_version)
        {
            return Err(StorageError::invalid());
        }
        if query.cursor.as_ref().is_some_and(|cursor| {
            cursor.observation_id <= 0
                || cursor.fetched_at < query.from
                || cursor.fetched_at >= query.to
        }) {
            return Err(StorageError::invalid());
        }
        let cursor_time = query
            .cursor
            .as_ref()
            .map(|cursor| cursor.fetched_at as i64)
            .unwrap_or(-1);
        let cursor_id = query
            .cursor
            .as_ref()
            .map(|cursor| cursor.observation_id)
            .unwrap_or(0);
        let connection = self.lock()?;
        let mut statement = connection.prepare("SELECT o.observation_id,o.collection_id,o.series_id,o.input_json,o.segment_id,o.boundary_reason,o.continuity_confidence FROM usage_observations o JOIN usage_series s ON s.series_id=o.series_id WHERE s.provider=?1 AND s.host=?2 AND s.account_id=?3 AND s.metric=?4 AND s.unit=?5 AND s.semantics_version=?6 AND o.fetched_at>=?7 AND o.fetched_at<?8 AND (o.fetched_at>?9 OR (o.fetched_at=?9 AND o.observation_id>?10)) ORDER BY o.fetched_at,o.observation_id LIMIT ?11")?;
        let rows = statement.query_map(
            params![
                query.account.provider,
                query.account.host,
                query.account.account_id,
                query.metric,
                query.unit,
                query.semantics_version,
                query.from as i64,
                query.to as i64,
                cursor_time,
                cursor_id,
                query.limit + 1
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )?;
        let mut observations = Vec::new();
        for row in rows {
            let (
                observation_id,
                collection_id,
                series_id,
                json,
                segment_id,
                boundary_reason,
                continuity_confidence,
            ) = row?;
            observations.push(UsageObservation {
                observation_id,
                collection_id,
                series_id,
                account: query.account.clone(),
                metric: serde_json::from_str(&json).map_err(|_| StorageError::database())?,
                segment_id,
                boundary_reason,
                continuity_confidence,
            });
        }
        let next_cursor = if observations.len() > query.limit as usize {
            observations.pop();
            observations.last().map(|observation| HistoryCursor {
                fetched_at: observation.metric.fetched_at,
                observation_id: observation.observation_id,
            })
        } else {
            None
        };
        Ok(HistoryPage {
            observations,
            next_cursor,
        })
    }

    /// Removes historical observations, collections, and their series. The
    /// independent latest cache survives this explicit history deletion.
    pub fn clear_history(&self, account: Option<&AccountKey>) -> Result<u64, StorageError> {
        if let Some(account) = account {
            validate_account(account)?;
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let count = if let Some(account) = account {
            let count: i64 = transaction.query_row("SELECT count(*) FROM usage_observations o JOIN usage_series s ON s.series_id=o.series_id WHERE s.provider=?1 AND s.host=?2 AND s.account_id=?3", params![account.provider, account.host, account.account_id], |row| row.get(0))?;
            transaction.execute(
                "DELETE FROM usage_collections WHERE provider=?1 AND host=?2 AND account_id=?3",
                params![account.provider, account.host, account.account_id],
            )?;
            transaction.execute(
                "DELETE FROM usage_series WHERE provider=?1 AND host=?2 AND account_id=?3",
                params![account.provider, account.host, account.account_id],
            )?;
            count
        } else {
            let count: i64 =
                transaction.query_row("SELECT count(*) FROM usage_observations", [], |row| {
                    row.get(0)
                })?;
            transaction.execute("DELETE FROM usage_collections", [])?;
            transaction.execute("DELETE FROM usage_series", [])?;
            count
        };
        transaction.commit()?;
        connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        Ok(count as u64)
    }
    pub fn clear_cache(&self, account: Option<&AccountKey>) -> Result<u64, StorageError> {
        let count = if let Some(account) = account {
            validate_account(account)?;
            self.lock()?.execute(
                "DELETE FROM usage_latest WHERE provider=?1 AND host=?2 AND account_id=?3",
                params![account.provider, account.host, account.account_id],
            )?
        } else {
            self.lock()?.execute("DELETE FROM usage_latest", [])?
        };
        Ok(count as u64)
    }
    /// Call at startup and periodically even when polling is disconnected.
    pub fn prune(&self, now: u64) -> Result<u64, StorageError> {
        if now > i64::MAX as u64 {
            return Err(StorageError::invalid());
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let settings = read_settings(&transaction)?;
        let count = prune_transaction(&transaction, now, settings.retention_days)?;
        transaction.commit()?;
        Ok(count)
    }
}

fn read_settings(connection: &Connection) -> Result<HistorySettings, StorageError> {
    Ok(connection.query_row(
        "SELECT enabled,retention_days FROM usage_settings WHERE singleton=1",
        [],
        |row| {
            Ok(HistorySettings {
                enabled: row.get(0)?,
                retention_days: row.get(1)?,
            })
        },
    )?)
}
fn valid_text(text: &str) -> bool {
    !text.is_empty() && text.len() <= 256 && !text.chars().any(char::is_control)
}
fn validate_account(account: &AccountKey) -> Result<(), StorageError> {
    if !valid_text(&account.provider)
        || !valid_text(&account.host)
        || !valid_text(&account.account_id)
        || account.host.contains('/')
        || account.host.contains('@')
    {
        return Err(StorageError::invalid());
    }
    Ok(())
}
fn validate_collection(
    collection_id: &str,
    account: &AccountKey,
    started_at: u64,
    finished_at: u64,
) -> Result<(), StorageError> {
    validate_account(account)?;
    if !valid_text(collection_id) || started_at > finished_at || finished_at > i64::MAX as u64 {
        return Err(StorageError::invalid());
    }
    Ok(())
}
fn exact_decimal(value: &str) -> Result<Decimal, StorageError> {
    // Expand exponent notation using only text operations. Parsing a mantissa
    // through a lossy Decimal constructor would silently round its precision.
    // The original spelling remains stored in ObservationInput.
    if value.is_empty() || value.len() > 96 || value.trim() != value {
        return Err(StorageError::invalid());
    }
    let mut parts = value.split(['e', 'E']);
    let mantissa = parts.next().ok_or_else(StorageError::invalid)?;
    let exponent = parts.next();
    if parts.next().is_some() {
        return Err(StorageError::invalid());
    }
    let (sign, unsigned) = if let Some(unsigned) = mantissa.strip_prefix('-') {
        ("-", unsigned)
    } else {
        ("", mantissa)
    };
    let mut mantissa_parts = unsigned.split('.');
    let integer = mantissa_parts.next().ok_or_else(StorageError::invalid)?;
    let fraction = mantissa_parts.next();
    if mantissa_parts.next().is_some()
        || integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.is_some_and(|fraction| {
            fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(StorageError::invalid());
    }
    let Some(exponent) = exponent else {
        return Decimal::from_str_exact(value).map_err(|_| StorageError::invalid());
    };
    let exponent: i32 = exponent.parse().map_err(|_| StorageError::invalid())?;
    if !(-96..=96).contains(&exponent) {
        return Err(StorageError::invalid());
    }
    let digits = format!("{integer}{}", fraction.unwrap_or_default());
    let point = integer.len() as i32 + exponent;
    let expanded = if point <= 0 {
        format!("{sign}0.{}{digits}", "0".repeat((-point) as usize))
    } else if point as usize >= digits.len() {
        format!(
            "{sign}{digits}{}",
            "0".repeat(point as usize - digits.len())
        )
    } else {
        let (integer, fraction) = digits.split_at(point as usize);
        format!("{sign}{integer}.{fraction}")
    };
    Decimal::from_str_exact(&expanded).map_err(|_| StorageError::invalid())
}
fn validate_observation(observation: &ObservationInput) -> Result<(), StorageError> {
    for text in [
        &observation.metric,
        &observation.unit,
        &observation.semantics_version,
        &observation.source_version,
    ] {
        if !valid_text(text) {
            return Err(StorageError::invalid());
        }
    }
    for text in [
        &observation.plan,
        &observation.period_id,
        &observation.period_start,
        &observation.period_end,
        &observation.reset_at,
    ]
    .into_iter()
    .flatten()
    {
        if !valid_text(text) {
            return Err(StorageError::invalid());
        }
    }
    if !matches!(
        observation.aggregation_kind.as_str(),
        "cumulative_counter" | "gauge" | "interval_total"
    ) || !matches!(
        observation.allowance_kind.as_str(),
        "finite" | "unlimited" | "unknown" | "not_applicable"
    ) || observation.fetched_at > i64::MAX as u64
        || observation
            .observed_at
            .is_some_and(|time| time > i64::MAX as u64)
        || observation.quality_flags.len() > 32
        || observation
            .quality_flags
            .iter()
            .any(|flag| !valid_text(flag))
    {
        return Err(StorageError::invalid());
    }
    for value in [
        &observation.used,
        &observation.entitlement,
        &observation.remaining,
        &observation.reported_percent_remaining,
    ]
    .into_iter()
    .flatten()
    {
        exact_decimal(value)?;
    }
    if observation
        .entitlement
        .as_deref()
        .map(exact_decimal)
        .transpose()?
        .is_some_and(|value| value.is_sign_negative())
        || (observation.aggregation_kind == "cumulative_counter"
            && observation
                .used
                .as_deref()
                .map(exact_decimal)
                .transpose()?
                .is_some_and(|value| value.is_sign_negative()))
        || (observation.allowance_kind == "finite" && observation.entitlement.is_none())
    {
        return Err(StorageError::invalid());
    }
    Ok(())
}
fn validate_projection(value: &Value) -> Result<(), StorageError> {
    fn walk(value: &Value, depth: usize) -> bool {
        if depth > 16 {
            return false;
        }
        match value {
            Value::Object(object) => object.iter().all(|(key, value)| {
                !matches!(
                    key.to_ascii_lowercase().as_str(),
                    "token"
                        | "access_token"
                        | "refresh_token"
                        | "authorization"
                        | "device_code"
                        | "client_secret"
                        | "messages"
                        | "conversation"
                        | "prompt"
                        | "response"
                ) && walk(value, depth + 1)
            }),
            Value::Array(values) => values.iter().all(|value| walk(value, depth + 1)),
            _ => true,
        }
    }
    if !value.is_object() || !walk(value, 0) {
        return Err(StorageError::invalid());
    }
    Ok(())
}
fn is_replay(
    transaction: &Transaction<'_>,
    collection_id: &str,
    account: &AccountKey,
) -> Result<bool, StorageError> {
    let owner: Option<(String,String,String)> = transaction.query_row("SELECT provider,host,account_id FROM usage_collections WHERE collection_id=?1 UNION ALL SELECT provider,host,account_id FROM usage_latest WHERE collection_id=?1 LIMIT 1", params![collection_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    match owner {
        Some((provider, host, account_id))
            if provider != account.provider
                || host != account.host
                || account_id != account.account_id =>
        {
            Err(StorageError::invalid())
        }
        Some(_) => Ok(true),
        None => Ok(false),
    }
}
fn prune_transaction(
    transaction: &Transaction<'_>,
    now: u64,
    retention_days: u32,
) -> Result<u64, StorageError> {
    let cutoff = now.saturating_sub(u64::from(retention_days) * DAY_MS) as i64;
    let count = transaction.execute(
        "DELETE FROM usage_observations WHERE fetched_at<?1",
        params![cutoff],
    )?;
    transaction.execute(
        "DELETE FROM usage_collections WHERE finished_at<?1",
        params![cutoff],
    )?;
    transaction.execute("DELETE FROM usage_series WHERE NOT EXISTS(SELECT 1 FROM usage_observations WHERE usage_observations.series_id=usage_series.series_id)", [])?;
    Ok(count as u64)
}

fn insert_observation(
    transaction: &Transaction<'_>,
    collection_id: &str,
    account: &AccountKey,
    input: &ObservationInput,
) -> Result<(), StorageError> {
    transaction.execute("INSERT INTO usage_series(provider,host,account_id,metric,unit,semantics_version,aggregation_kind) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(provider,host,account_id,metric,unit,semantics_version) DO NOTHING",
        params![account.provider, account.host, account.account_id, input.metric, input.unit, input.semantics_version, input.aggregation_kind])?;
    let (series_id,aggregation_kind): (i64,String) = transaction.query_row("SELECT series_id,aggregation_kind FROM usage_series WHERE provider=?1 AND host=?2 AND account_id=?3 AND metric=?4 AND unit=?5 AND semantics_version=?6",
        params![account.provider,account.host,account.account_id,input.metric,input.unit,input.semantics_version], |row| Ok((row.get(0)?,row.get(1)?)))?;
    if aggregation_kind != input.aggregation_kind {
        return Err(StorageError::invalid());
    }
    let previous: Option<(String,String)> = transaction.query_row("SELECT input_json,segment_id FROM usage_observations WHERE series_id=?1 ORDER BY fetched_at DESC,observation_id DESC LIMIT 1", params![series_id], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
    let previous = previous
        .map(|(json, segment)| {
            Ok::<_, StorageError>((
                serde_json::from_str::<ObservationInput>(&json)
                    .map_err(|_| StorageError::database())?,
                segment,
            ))
        })
        .transpose()?;
    let latest_series: Option<i64> = transaction.query_row(
        "SELECT s.series_id FROM usage_series s JOIN usage_observations o ON s.series_id=o.series_id WHERE s.provider=?1 AND s.host=?2 AND s.account_id=?3 AND s.metric=?4 ORDER BY o.fetched_at DESC,o.observation_id DESC LIMIT 1",
        params![account.provider,account.host,account.account_id,input.metric], |row| row.get(0)).optional()?;
    let (segment_id, reason, confidence) =
        if latest_series.is_some_and(|latest| latest != series_id) {
            (
                Uuid::new_v4().to_string(),
                Some("series_changed".to_owned()),
                "unknown".to_owned(),
            )
        } else {
            continuity(input, previous.as_ref())?
        };
    let input_json = serde_json::to_string(input).map_err(|_| StorageError::invalid())?;
    transaction.execute("INSERT INTO usage_observations(collection_id,series_id,fetched_at,input_json,segment_id,boundary_reason,continuity_confidence) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![collection_id,series_id,input.fetched_at as i64,input_json,segment_id,reason,confidence])?;
    Ok(())
}
fn decimal_equal(left: &Option<String>, right: &Option<String>) -> Result<bool, StorageError> {
    Ok(left.as_deref().map(exact_decimal).transpose()?
        == right.as_deref().map(exact_decimal).transpose()?)
}
fn continuity(
    input: &ObservationInput,
    previous: Option<&(ObservationInput, String)>,
) -> Result<(String, Option<String>, String), StorageError> {
    let fresh = |reason: &str| {
        (
            Uuid::new_v4().to_string(),
            Some(reason.to_owned()),
            "unknown".to_owned(),
        )
    };
    let Some((prior, segment)) = previous else {
        return Ok(fresh("initial"));
    };
    if input.fetched_at <= prior.fetched_at {
        return Ok(fresh("out_of_order"));
    }
    if input.reset_at != prior.reset_at
        || input.period_id != prior.period_id
        || input.period_start != prior.period_start
        || input.period_end != prior.period_end
    {
        return Ok(fresh("period_changed"));
    }
    if input.plan != prior.plan
        || input.allowance_kind != prior.allowance_kind
        || !decimal_equal(&input.entitlement, &prior.entitlement)?
    {
        return Ok(fresh("allowance_changed"));
    }
    if input.aggregation_kind == "cumulative_counter" {
        if let (Some(used), Some(prior_used)) = (&input.used, &prior.used) {
            if exact_decimal(used)? < exact_decimal(prior_used)? {
                return Ok(fresh("counter_decreased"));
            }
        }
    }
    if input.fetched_at - prior.fetched_at > MAX_GAP_MS {
        return Ok(fresh("gap"));
    }
    if !input.semantics_verified || !prior.semantics_verified {
        return Ok(fresh("semantics_unverified"));
    }
    if input.used.is_none() || prior.used.is_none() {
        return Ok(fresh("metric_unavailable"));
    }
    if input.source_version != prior.source_version
        || !input.quality_flags.is_empty()
        || !prior.quality_flags.is_empty()
    {
        return Ok(fresh("quality_unknown"));
    }
    // An unchanged next-reset value alone does not prove a reporting period.
    if input.period_id.is_none() && (input.period_start.is_none() || input.period_end.is_none()) {
        return Ok(fresh("period_unknown"));
    }
    Ok((segment.clone(), None, "confirmed".to_owned()))
}

const MIGRATION_1: &str = "
CREATE TABLE usage_settings(singleton INTEGER PRIMARY KEY CHECK(singleton=1),enabled INTEGER NOT NULL CHECK(enabled IN(0,1)),retention_days INTEGER NOT NULL CHECK(retention_days BETWEEN 1 AND 3650));
INSERT INTO usage_settings VALUES(1,1,90);
CREATE TABLE usage_series(series_id INTEGER PRIMARY KEY AUTOINCREMENT,provider TEXT NOT NULL,host TEXT NOT NULL,account_id TEXT NOT NULL,metric TEXT NOT NULL,unit TEXT NOT NULL,semantics_version TEXT NOT NULL,aggregation_kind TEXT NOT NULL,UNIQUE(provider,host,account_id,metric,unit,semantics_version));
CREATE TABLE usage_collections(collection_id TEXT PRIMARY KEY,provider TEXT NOT NULL,host TEXT NOT NULL,account_id TEXT NOT NULL,started_at INTEGER NOT NULL,finished_at INTEGER NOT NULL,status TEXT NOT NULL CHECK(status IN('success','failure')),error_category TEXT,projection_json TEXT,CHECK(finished_at>=started_at),CHECK((status='success' AND error_category IS NULL AND projection_json IS NOT NULL) OR (status='failure' AND error_category IS NOT NULL AND projection_json IS NULL)));
CREATE TABLE usage_observations(observation_id INTEGER PRIMARY KEY AUTOINCREMENT,collection_id TEXT NOT NULL REFERENCES usage_collections(collection_id) ON DELETE CASCADE,series_id INTEGER NOT NULL REFERENCES usage_series(series_id) ON DELETE CASCADE,fetched_at INTEGER NOT NULL,input_json TEXT NOT NULL,segment_id TEXT NOT NULL,boundary_reason TEXT,continuity_confidence TEXT NOT NULL CHECK(continuity_confidence IN('unknown','confirmed')),UNIQUE(collection_id,series_id));
CREATE TABLE usage_latest(provider TEXT NOT NULL,host TEXT NOT NULL,account_id TEXT NOT NULL,collection_id TEXT NOT NULL,fetched_at INTEGER NOT NULL,projection_json TEXT NOT NULL,PRIMARY KEY(provider,host,account_id));
CREATE TRIGGER usage_observations_immutable BEFORE UPDATE ON usage_observations BEGIN SELECT RAISE(ABORT,'Usage observations are immutable'); END;
";
const MIGRATION_2: &str = "
CREATE INDEX usage_history_range ON usage_observations(series_id,fetched_at,observation_id);
CREATE INDEX usage_observation_retention ON usage_observations(fetched_at);
CREATE INDEX usage_collection_retention ON usage_collections(finished_at);
CREATE INDEX usage_collection_account ON usage_collections(provider,host,account_id,finished_at);
";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn account() -> AccountKey {
        AccountKey {
            provider: "github-copilot".into(),
            host: "api.github.com".into(),
            account_id: "42".into(),
        }
    }
    fn input(fetched_at: u64) -> ObservationInput {
        ObservationInput {
            metric: "premium_interactions".into(),
            unit: "credits".into(),
            semantics_version: "v1".into(),
            aggregation_kind: "cumulative_counter".into(),
            semantics_verified: false,
            fetched_at,
            observed_at: None,
            plan: Some("enterprise".into()),
            entitlement: Some("2000000".into()),
            allowance_kind: "finite".into(),
            used: Some("722342".into()),
            remaining: Some("1277551.100000000000000000001".into()),
            reported_percent_remaining: Some("63.8".into()),
            overage_permitted: Some(true),
            period_id: None,
            period_start: None,
            period_end: None,
            reset_at: Some("2026-10-01T00:00:00.000Z".into()),
            source_version: "copilot_internal/v1".into(),
            quality_flags: vec!["semantics_unverified".into()],
        }
    }
    fn query(account: AccountKey) -> HistoryQuery {
        HistoryQuery {
            account,
            metric: "premium_interactions".into(),
            unit: "credits".into(),
            semantics_version: "v1".into(),
            from: 0,
            to: i64::MAX as u64,
            limit: 500,
            cursor: None,
        }
    }
    fn record(storage: &Storage, id: &str, account: &AccountKey, input: ObservationInput) -> bool {
        storage
            .record_success(
                id,
                account,
                input.fetched_at,
                input.fetched_at,
                std::slice::from_ref(&input),
                &json!({"fetched_at":input.fetched_at,"used":input.used}),
            )
            .unwrap()
    }
    fn count(storage: &Storage, table: &str) -> i64 {
        // Test-owned table names only.
        storage
            .lock()
            .unwrap()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }
    #[test]
    fn storage_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Storage>();
    }
    #[test]
    fn immutable_exact_samples_keep_unchanged_polls_and_reject_replays() {
        let storage = Storage::in_memory().unwrap();
        assert!(record(&storage, "poll-1", &account(), input(100)));
        assert!(!record(&storage, "poll-1", &account(), input(100)));
        assert!(record(&storage, "poll-2", &account(), input(200)));
        let observations = storage.query(query(account())).unwrap().observations;
        assert_eq!(observations.len(), 2);
        assert_eq!(
            observations[0].metric.remaining.as_deref(),
            Some("1277551.100000000000000000001")
        );
        assert_eq!(observations[0].metric, input(100));
        assert_ne!(
            observations[0].observation_id,
            observations[1].observation_id
        );
        assert!(storage
            .lock()
            .unwrap()
            .execute("UPDATE usage_observations SET fetched_at=10", [])
            .is_err());
        assert_eq!(
            storage.latest(&account()).unwrap().unwrap()["fetched_at"],
            json!(200)
        );
    }
    #[test]
    fn failure_and_cache_only_polls_never_create_observations() {
        let storage = Storage::in_memory().unwrap();
        record(&storage, "success", &account(), input(100));
        assert!(storage
            .record_failure("failure", &account(), 200, 201, "network")
            .unwrap());
        assert!(!storage
            .record_failure("failure", &account(), 200, 201, "network")
            .unwrap());
        assert_eq!(count(&storage, "usage_observations"), 1);
        assert_eq!(count(&storage, "usage_collections"), 2);
        assert_eq!(
            storage.latest(&account()).unwrap().unwrap()["fetched_at"],
            json!(100)
        );
        storage.set_settings(false, 90).unwrap();
        assert_eq!(count(&storage, "usage_observations"), 0);
        record(&storage, "disabled", &account(), input(300));
        assert!(!storage
            .record_failure("disabled-failure", &account(), 400, 400, "network")
            .unwrap());
        assert_eq!(count(&storage, "usage_observations"), 0);
        assert_eq!(count(&storage, "usage_collections"), 0);
        assert_eq!(
            storage.latest(&account()).unwrap().unwrap()["fetched_at"],
            json!(300)
        );
        assert!(storage
            .record_failure("secret", &account(), 500, 500, "Bearer secret")
            .is_err());
    }
    #[test]
    fn transaction_rolls_back_every_metric_and_latest_on_series_conflict() {
        let storage = Storage::in_memory().unwrap();
        record(&storage, "first", &account(), input(100));
        let mut other = input(200);
        other.metric = "new-metric".into();
        let mut invalid = input(200);
        invalid.aggregation_kind = "gauge".into();
        assert!(storage
            .record_success(
                "bad",
                &account(),
                200,
                200,
                &[other, invalid],
                &json!({"fetched_at":200})
            )
            .is_err());
        assert_eq!(count(&storage, "usage_series"), 1);
        assert_eq!(count(&storage, "usage_collections"), 1);
        assert_eq!(count(&storage, "usage_observations"), 1);
        assert_eq!(
            storage.latest(&account()).unwrap().unwrap()["fetched_at"],
            json!(100)
        );
    }
    #[test]
    fn ordered_cache_and_keyset_pages_keep_same_timestamp_observations() {
        let storage = Storage::in_memory().unwrap();
        for (id, time) in [
            ("new", 300),
            ("old-a", 100),
            ("old-b", 100),
            ("middle", 200),
        ] {
            record(&storage, id, &account(), input(time));
        }
        assert_eq!(
            storage.latest(&account()).unwrap().unwrap()["fetched_at"],
            json!(300)
        );
        let mut range = query(account());
        range.from = 100;
        range.to = 300;
        range.limit = 2;
        let first = storage.query(range.clone()).unwrap();
        assert_eq!(
            first
                .observations
                .iter()
                .map(|sample| sample.metric.fetched_at)
                .collect::<Vec<_>>(),
            vec![100, 100]
        );
        range.cursor = first.next_cursor;
        let last = storage.query(range).unwrap();
        assert_eq!(last.observations.len(), 1);
        assert_eq!(last.observations[0].metric.fetched_at, 200);
        assert!(last.next_cursor.is_none());
        let mut invalid = query(account());
        invalid.limit = 501;
        assert!(storage.query(invalid).is_err());
        assert_eq!(count(&storage, "usage_observations"), 4);
    }
    #[test]
    fn boundaries_prevent_false_consumption_deltas() {
        let storage = Storage::in_memory().unwrap();
        let mut current = input(100);
        current.semantics_verified = true;
        current.quality_flags.clear();
        current.period_id = Some("verified-period".into());
        record(&storage, "first", &account(), current.clone());
        current.fetched_at += 100;
        record(&storage, "continuous", &account(), current.clone());
        current.fetched_at += 100;
        current.used = Some("1".into());
        record(&storage, "decrease", &account(), current.clone());
        current.fetched_at += 100;
        current.reset_at = Some("2026-11-01T00:00:00.000Z".into());
        record(&storage, "reset", &account(), current.clone());
        current.fetched_at += 100;
        current.entitlement = Some("3000000".into());
        record(&storage, "allowance", &account(), current.clone());
        current.fetched_at += MAX_GAP_MS + 1;
        record(&storage, "gap", &account(), current.clone());
        let samples = storage.query(query(account())).unwrap().observations;
        assert_eq!(samples[0].segment_id, samples[1].segment_id);
        assert_eq!(samples[1].continuity_confidence, "confirmed");
        for (index, reason) in [
            (2, "counter_decreased"),
            (3, "period_changed"),
            (4, "allowance_changed"),
            (5, "gap"),
        ] {
            assert_eq!(samples[index].boundary_reason.as_deref(), Some(reason));
            assert_ne!(samples[index].segment_id, samples[index - 1].segment_id);
            assert_eq!(samples[index].continuity_confidence, "unknown");
        }
    }
    #[test]
    fn semantics_unit_and_account_identities_remain_separate() {
        let storage = Storage::in_memory().unwrap();
        record(&storage, "first", &account(), input(100));
        let mut changed = input(200);
        changed.semantics_version = "v2".into();
        record(&storage, "v2", &account(), changed.clone());
        changed.unit = "requests".into();
        changed.fetched_at = 300;
        record(&storage, "requests", &account(), changed);
        let another = AccountKey {
            account_id: "84".into(),
            ..account()
        };
        record(&storage, "other", &another, input(400));
        assert_eq!(count(&storage, "usage_series"), 4);
        assert_eq!(
            storage.query(query(account())).unwrap().observations.len(),
            1
        );
        assert_eq!(
            storage
                .query(query(another.clone()))
                .unwrap()
                .observations
                .len(),
            1
        );
        assert!(storage
            .record_success("first", &another, 100, 100, &[input(100)], &json!({}))
            .is_err());
        let mut newer = query(account());
        newer.semantics_version = "v2".into();
        assert_eq!(
            storage.query(newer).unwrap().observations[0]
                .boundary_reason
                .as_deref(),
            Some("series_changed")
        );
    }
    #[test]
    fn returning_to_an_earlier_series_cannot_rejoin_its_segment() {
        let storage = Storage::in_memory().unwrap();
        let mut sample = input(100);
        sample.semantics_verified = true;
        sample.period_id = Some("period".into());
        sample.quality_flags.clear();
        record(&storage, "v1-first", &account(), sample.clone());
        sample.fetched_at = 200;
        sample.semantics_version = "v2".into();
        record(&storage, "v2", &account(), sample.clone());
        sample.fetched_at = 300;
        sample.semantics_version = "v1".into();
        record(&storage, "v1-return", &account(), sample);
        let samples = storage.query(query(account())).unwrap().observations;
        assert_eq!(samples.len(), 2);
        assert_ne!(samples[0].segment_id, samples[1].segment_id);
        assert_eq!(
            samples[1].boundary_reason.as_deref(),
            Some("series_changed")
        );
        assert_eq!(samples[1].continuity_confidence, "unknown");
    }
    #[test]
    fn exponent_numbers_are_exact_and_retain_their_original_spelling() {
        for (scientific, fixed) in [
            ("1e6", "1000000"),
            ("1.2e-3", "0.0012"),
            ("-1.25E+2", "-125"),
            ("1e-28", "0.0000000000000000000000000001"),
            (
                "1277551100000000000000000001e-21",
                "1277551.100000000000000000001",
            ),
            (
                "1.2345678901234567890123456789e1",
                "12.345678901234567890123456789",
            ),
        ] {
            assert_eq!(
                exact_decimal(scientific).unwrap(),
                exact_decimal(fixed).unwrap()
            );
        }
        for invalid in [
            "1e-29",
            "1e29",
            "1.234567890123456789012345678901e-1",
            "1e9999999999999999999999",
            "1e97",
            "1e-97",
            "1e",
            "1e1e2",
            "1_0e1",
            "1.e1",
        ] {
            assert!(exact_decimal(invalid).is_err(), "Accepted {invalid}");
        }
        let storage = Storage::in_memory().unwrap();
        let mut scientific = input(100);
        scientific.used = Some("7.22342e5".into());
        scientific.remaining = Some("1277551100000000000000000001e-21".into());
        record(&storage, "scientific", &account(), scientific.clone());
        let saved = storage
            .query(query(account()))
            .unwrap()
            .observations
            .pop()
            .unwrap();
        assert_eq!(saved.metric.used, scientific.used);
        assert_eq!(saved.metric.remaining, scientific.remaining);
    }
    #[test]
    fn unknown_metrics_and_unlimited_allowance_are_not_fabricated_zeroes() {
        let storage = Storage::in_memory().unwrap();
        let mut unknown = input(100);
        unknown.used = None;
        unknown.entitlement = None;
        unknown.allowance_kind = "unlimited".into();
        record(&storage, "unlimited", &account(), unknown);
        let sample = storage
            .query(query(account()))
            .unwrap()
            .observations
            .pop()
            .unwrap();
        assert!(sample.metric.used.is_none());
        assert!(sample.metric.entitlement.is_none());
        assert_eq!(sample.metric.allowance_kind, "unlimited");
        assert!(storage
            .record_success(
                "bad-secret",
                &account(),
                200,
                200,
                &[input(200)],
                &json!({"nested":{"access_token":"secret"}})
            )
            .is_err());
        assert!(exact_decimal("0.00000000000000000000000000001").is_err());
    }
    #[test]
    fn retention_and_explicit_deletion_preserve_independent_cache() {
        let storage = Storage::in_memory().unwrap();
        assert_eq!(
            storage.settings().unwrap(),
            HistorySettings {
                enabled: true,
                retention_days: 90
            }
        );
        record(&storage, "old", &account(), input(DAY_MS));
        record(&storage, "boundary", &account(), input(2 * DAY_MS));
        storage
            .record_failure("old-failure", &account(), DAY_MS, DAY_MS, "network")
            .unwrap();
        assert_eq!(storage.prune(92 * DAY_MS).unwrap(), 1);
        assert_eq!(count(&storage, "usage_observations"), 1);
        assert_eq!(count(&storage, "usage_collections"), 1);
        let another = AccountKey {
            account_id: "84".into(),
            ..account()
        };
        record(&storage, "other", &another, input(2 * DAY_MS));
        assert_eq!(storage.clear_history(Some(&account())).unwrap(), 1);
        assert!(storage.latest(&account()).unwrap().is_some());
        assert_eq!(
            storage
                .query(query(another.clone()))
                .unwrap()
                .observations
                .len(),
            1
        );
        assert_eq!(storage.clear_cache(Some(&another)).unwrap(), 1);
        assert_eq!(
            storage
                .query(query(another.clone()))
                .unwrap()
                .observations
                .len(),
            1
        );
        assert!(storage.rebuild_cache(&another).unwrap());
        assert!(storage.latest(&another).unwrap().is_some());
        assert_eq!(storage.clear_history(None).unwrap(), 1);
        assert_eq!(count(&storage, "usage_collections"), 0);
        assert_eq!(count(&storage, "usage_series"), 0);
        assert_eq!(storage.clear_cache(None).unwrap(), 2);
        assert!(storage.latest(&account()).unwrap().is_none());
        assert!(storage.set_settings(true, 0).is_err());
        storage.set_settings(false, 1).unwrap();
        assert_eq!(storage.settings().unwrap().retention_days, 1);
    }
    #[test]
    fn restart_and_version_one_migration_preserve_history() {
        let path =
            std::env::temp_dir().join(format!("luma-storage-test-{}.sqlite", Uuid::new_v4()));
        {
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(MIGRATION_1).unwrap();
            connection.pragma_update(None, "user_version", 1).unwrap();
        }
        {
            let storage = Storage::open(&path).unwrap();
            storage.set_settings(true, 7).unwrap();
            record(&storage, "persisted", &account(), input(100));
        }
        {
            let storage = Storage::open(&path).unwrap();
            assert_eq!(storage.settings().unwrap().retention_days, 7);
            let samples = storage.query(query(account())).unwrap().observations;
            assert_eq!(samples.len(), 1);
            assert_eq!(samples[0].metric, input(100));
            assert!(storage.latest(&account()).unwrap().is_some());
            assert!(!record(&storage, "persisted", &account(), input(100)));
            assert_eq!(
                storage
                    .lock()
                    .unwrap()
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                SCHEMA_VERSION
            );
        }
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn future_database_schema_is_rejected_without_rewriting_version() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        assert!(matches!(
            Storage::initialize(connection),
            Err(StorageError {
                code: "usage_schema_newer",
                ..
            })
        ));
    }
}
