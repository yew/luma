//! Native GitHub device flow. Secrets never implement Serialize or Debug.
use crate::storage::{AccountKey, ObservationInput, Storage};
use reqwest::{header::HeaderMap, Client, Response};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, RwLock,
};
static STATUS_SEQUENCE: AtomicU64 = AtomicU64::new(0);
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{Emitter, State};
use tokio::sync::{watch, Mutex};

#[derive(Clone, Serialize, Debug, PartialEq)]
pub struct GitHubError {
    pub code: String,
    pub message: String,
    pub retry_after: Option<u64>,
}
impl GitHubError {
    fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retry_after: None,
        }
    }
    fn cancelled() -> Self {
        Self::new("cancelled", "GitHub operation was cancelled.")
    }
    fn storage() -> Self {
        Self::new(
            "secure_storage",
            "Cannot access the OS credential store. No plaintext fallback is used.",
        )
    }
    fn schema() -> Self {
        Self::new(
            "schema",
            "Unsupported GitHub response. Last valid usage is retained.",
        )
    }
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Account {
    pub id: u64,
    pub login: String,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct PremiumQuota {
    pub entitlement: Option<serde_json::Number>,
    pub credits_used: Option<serde_json::Number>,
    pub quota_remaining: Option<serde_json::Number>,
    pub percent_remaining: Option<serde_json::Number>,
    pub overage_permitted: Option<bool>,
    pub unlimited: Option<bool>,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct CopilotSnapshot {
    pub account_id: u64,
    pub login: String,
    pub plan: Option<String>,
    pub reset_at: Option<String>,
    pub fetched_at: u64,
    pub premium: PremiumQuota,
    pub used_percent: Option<f64>,
}
#[derive(Clone, Serialize)]
pub struct AuthStatus {
    pub revision: u64,
    /// Earliest permitted manual refresh, including provider cooldown/backoff.
    pub next_refresh_at: Option<u64>,
    pub state: String,
    pub account: Option<Account>,
    pub snapshot: Option<CopilotSnapshot>,
    pub error: Option<GitHubError>,
}
#[derive(Serialize)]
pub struct DeviceAuthorization {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}
#[derive(Serialize)]
pub struct PollResult {
    pub state: String,
    pub retry_after: Option<u64>,
    pub status: Option<AuthStatus>,
}
impl PollResult {
    fn pending(seconds: u64) -> Self {
        Self {
            state: "pending".into(),
            retry_after: Some(seconds),
            status: None,
        }
    }
}
#[derive(Deserialize)]
struct OAuthConfig {
    client_id: String,
    authorization_flow: String,
    scopes: Vec<String>,
}
#[derive(Deserialize)]
struct DeviceResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: Option<u64>,
}
struct DeviceSession {
    code: String,
    expires: Instant,
    interval: u64,
    next_poll: Instant,
}
// Only serialized into the OS credential store; never exposed via commands.
#[derive(Clone, Serialize, Deserialize)]
struct StoredToken {
    access_token: String,
    expires_at: Option<u64>,
    #[serde(default)]
    account: Option<Account>,
}
fn restore_token(value: &str) -> Result<StoredToken, GitHubError> {
    if value.starts_with('{') {
        let token: StoredToken = serde_json::from_str(value).map_err(|_| GitHubError::storage())?;
        if token.access_token.is_empty() {
            return Err(GitHubError::storage());
        }
        Ok(token)
    } else {
        Ok(StoredToken {
            access_token: value.to_owned(),
            expires_at: None,
            account: None,
        })
    }
}
// Keep OS-store effects behind a narrow boundary so failure paths can be tested
// without ever reading, overwriting, or deleting a developer's credentials.
trait CredentialStore: Send + Sync {
    fn load(&self) -> Result<Option<StoredToken>, GitHubError>;
    fn save(&self, token: &StoredToken) -> Result<(), GitHubError>;
    fn delete(&self) -> Result<(), GitHubError>;
}
struct SystemCredentialStore;
impl CredentialStore for SystemCredentialStore {
    fn load(&self) -> Result<Option<StoredToken>, GitHubError> {
        match credential()?.get_password() {
            Ok(value) if !value.is_empty() => restore_token(&value).map(Some),
            Ok(_) | Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(GitHubError::storage()),
        }
    }
    fn save(&self, token: &StoredToken) -> Result<(), GitHubError> {
        credential()?
            .set_password(&serde_json::to_string(token).map_err(|_| GitHubError::storage())?)
            .map_err(|_| GitHubError::storage())
    }
    fn delete(&self) -> Result<(), GitHubError> {
        match credential()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(GitHubError::storage()),
        }
    }
}
fn token_expiration(raw: &serde_json::Value, now: u64) -> Result<Option<u64>, GitHubError> {
    match raw.get("expires_in") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => {
            let seconds = value.as_u64().ok_or_else(GitHubError::schema)?;
            // Zero does not establish a positive lifetime; rely on server validation.
            if seconds == 0 {
                return Ok(None);
            }
            let duration = seconds.checked_mul(1000).ok_or_else(GitHubError::schema)?;
            Ok(Some(
                now.checked_add(duration).ok_or_else(GitHubError::schema)?,
            ))
        }
    }
}
struct Inner {
    token: Option<StoredToken>,
    loaded: bool,
    account: Option<Account>,
    snapshot: Option<CopilotSnapshot>,
    error: Option<GitHubError>,
    device: Option<DeviceSession>,
    busy: bool,
    generation: u64,
    next_refresh: Option<Instant>,
    failures: u32,
    last_successful_poll: Option<Instant>,
}
impl Inner {
    fn authentication_error(&mut self, error: GitHubError) {
        // An unsuccessful replacement authorization does not change the saved
        // connection's validity. In particular, it cannot clear a revoked token.
        if self.token.is_none() {
            self.error = Some(error);
        }
    }
    fn status(&self) -> AuthStatus {
        let state = if self
            .error
            .as_ref()
            .is_some_and(|e| e.code == "reauth_required")
        {
            "reauth_required"
        } else if self.token.is_some() && self.account.is_some() {
            "connected"
        } else if self.error.is_some() {
            "error"
        } else {
            "disconnected"
        };
        AuthStatus {
            revision: STATUS_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1,
            next_refresh_at: self
                .next_refresh
                .map(|deadline| deadline_epoch_millis(deadline, Instant::now(), epoch_millis())),
            state: state.into(),
            account: self.account.clone(),
            snapshot: self.snapshot.clone(),
            error: self.error.clone(),
        }
    }
}
pub struct GitHubService {
    client: RwLock<Client>,
    storage: Arc<Storage>,
    client_id: String,
    credentials: Arc<dyn CredentialStore>,
    inner: Mutex<Inner>,
    cancel: watch::Sender<u64>,
    restore: Mutex<()>,
}
fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn credential() -> Result<keyring::Entry, GitHubError> {
    keyring::Entry::new("dev.luma.github", "oauth-token").map_err(|_| GitHubError::storage())
}
fn seconds_until(deadline: Instant, now: Instant) -> u64 {
    let remaining = deadline.saturating_duration_since(now);
    remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0)
}
impl GitHubService {
    pub fn new(
        storage: Arc<Storage>,
        proxy: &crate::network::ProxySettings,
    ) -> Result<Self, GitHubError> {
        Self::with_credentials(storage, proxy, Arc::new(SystemCredentialStore))
    }
    fn with_credentials(
        storage: Arc<Storage>,
        proxy: &crate::network::ProxySettings,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self, GitHubError> {
        let config: OAuthConfig =
            serde_json::from_str(include_str!("../../config/github-oauth.json")).map_err(|_| {
                GitHubError::new("configuration", "Invalid GitHub OAuth configuration.")
            })?;
        if config.client_id.trim().is_empty()
            || config.authorization_flow != "device"
            || !config.scopes.is_empty()
        {
            return Err(GitHubError::new(
                "configuration",
                "OAuth requires Luma's client ID, device flow, and the validated empty scope list.",
            ));
        }
        let client = crate::network::build_client(proxy).map_err(|_| {
            GitHubError::new("network", "Cannot initialize HTTPS or proxy configuration.")
        })?;
        let (cancel, _) = watch::channel(0);
        Ok(Self {
            client: RwLock::new(client),
            storage,
            client_id: config.client_id,
            credentials,
            cancel,
            restore: Mutex::new(()),
            inner: Mutex::new(Inner {
                token: None,
                loaded: false,
                account: None,
                snapshot: None,
                error: None,
                device: None,
                busy: false,
                generation: 0,
                next_refresh: None,
                failures: 0,
                last_successful_poll: None,
            }),
        })
    }
    fn client(&self) -> Result<Client, GitHubError> {
        self.client
            .read()
            .map(|client| client.clone())
            .map_err(|_| GitHubError::new("network", "Cannot access network configuration."))
    }
    pub fn replace_client(
        &self,
        client: Client,
        persist: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let mut active = self
            .client
            .write()
            .map_err(|_| "Cannot update network configuration.".to_string())?;
        persist()?;
        *active = client;
        Ok(())
    }
    fn persist(
        &self,
        collection_id: &str,
        started_at: u64,
        account: &Account,
        snapshot: &CopilotSnapshot,
    ) -> Result<(), GitHubError> {
        let premium = &snapshot.premium;
        let observation = ObservationInput {
            metric: "premium_interactions".into(),
            unit: "provider_quota_credit".into(),
            semantics_version: "copilot-premium-v1".into(),
            semantics_verified: false,
            aggregation_kind: "cumulative_counter".into(),
            allowance_kind: if premium.unlimited == Some(true) {
                "unlimited"
            } else if premium.entitlement.is_some() {
                "finite"
            } else {
                "unknown"
            }
            .into(),
            source_version: "copilot_internal/v1".into(),
            quality_flags: vec!["semantics_unverified".into(), "collection_time".into()],
            fetched_at: snapshot.fetched_at,
            observed_at: None,
            plan: snapshot.plan.clone(),
            entitlement: premium.entitlement.as_ref().map(ToString::to_string),
            used: premium.credits_used.as_ref().map(ToString::to_string),
            remaining: premium.quota_remaining.as_ref().map(ToString::to_string),
            reported_percent_remaining: premium.percent_remaining.as_ref().map(ToString::to_string),
            overage_permitted: premium.overage_permitted,
            period_id: None,
            period_start: None,
            period_end: None,
            reset_at: snapshot.reset_at.clone(),
        };
        if premium.credits_used.is_none() {
            return self
                .storage
                .record_failure(
                    collection_id,
                    &account_key(account),
                    started_at,
                    snapshot.fetched_at,
                    "metric_unavailable",
                )
                .map(|_| ())
                .map_err(|_| history_error());
        }
        let projection = serde_json::to_value(snapshot).map_err(|_| history_error())?;
        self.storage
            .record_success(
                collection_id,
                &account_key(account),
                started_at,
                snapshot.fetched_at,
                &[observation],
                &projection,
            )
            .map(|_| ())
            .map_err(|_| history_error())
    }
    pub async fn background_tick(&self, app: &tauri::AppHandle) {
        let interval = crate::preferences::current_refresh_interval(app);
        let should_refresh = {
            let inner = self.inner.lock().await;
            inner.loaded
                && inner.token.is_some()
                && !inner.busy
                && inner.device.is_none()
                && !inner
                    .error
                    .as_ref()
                    .is_some_and(|e| e.code == "reauth_required")
                && inner.next_refresh.is_none_or(|next| next <= Instant::now())
                && (inner.error.as_ref().is_some_and(|e| {
                    !matches!(e.code.as_str(), "history_storage" | "cached_response")
                }) || inner
                    .last_successful_poll
                    .is_none_or(|last| last.elapsed() >= Duration::from_secs(interval)))
        };
        if should_refresh {
            let _ = self.refresh().await;
            let _ = app.emit("github-status", self.inner.lock().await.status());
        }
    }
    pub async fn clear_cached_snapshot(
        &self,
        app: &tauri::AppHandle,
        account: Option<&AccountKey>,
    ) -> Result<u64, crate::storage::StorageError> {
        let (count, status) = self.clear_cache_inner(account).await?;
        let _ = app.emit("github-status", status);
        Ok(count)
    }
    async fn clear_cache_inner(
        &self,
        account: Option<&AccountKey>,
    ) -> Result<(u64, AuthStatus), crate::storage::StorageError> {
        // Refresh writes use this same lock; invalidate pending replies before deleting.
        let mut inner = self.inner.lock().await;
        let current = account.is_none()
            || inner
                .account
                .as_ref()
                .is_some_and(|current| account == Some(&account_key(current)));
        if current {
            inner.generation = inner.generation.wrapping_add(1);
            self.cancel.send_replace(inner.generation);
            inner.busy = false;
            inner.device = None;
        }
        let count = self.storage.clear_cache(account)?;
        if current {
            inner.snapshot = None;
            inner.last_successful_poll = None;
            // Clearing local cache cannot bypass the provider's existing wait.
            inner.next_refresh = Some(
                inner
                    .next_refresh
                    .unwrap_or_else(Instant::now)
                    .max(deadline_after(30)),
            );
        }
        Ok((count, inner.status()))
    }
    async fn reserve(&self) -> Result<(u64, watch::Receiver<u64>), GitHubError> {
        let mut inner = self.inner.lock().await;
        if inner.busy {
            return Err(GitHubError::new(
                "busy",
                "A GitHub request is already in progress.",
            ));
        }
        inner.busy = true;
        Ok((inner.generation, self.cancel.subscribe()))
    }
    async fn request(
        &self,
        request: reqwest::RequestBuilder,
        cancel: &mut watch::Receiver<u64>,
    ) -> Result<serde_json::Value, GitHubError> {
        self.request_reply(request, cancel, false)
            .await?
            .body
            .ok_or_else(GitHubError::schema)
    }
    async fn request_reply(
        &self,
        request: reqwest::RequestBuilder,
        cancel: &mut watch::Receiver<u64>,
        allow_not_modified: bool,
    ) -> Result<UsageReply, GitHubError> {
        tokio::select! {
            biased;
            _ = cancel.changed() => Err(GitHubError::cancelled()),
            result = async {
                let response = request.header("Accept", "application/json").send().await
                    .map_err(|_| GitHubError::new("network", "Cannot reach GitHub. Check the connection and retry."))?;
                let cache = cache_policy(response.headers(), SystemTime::now());
                if response.status() == reqwest::StatusCode::NOT_MODIFIED && allow_not_modified {
                    return Ok(UsageReply { body: None, cache });
                }
                if !response.status().is_success() { return Err(http_error(&response)); }
                let bytes = bounded_body(response).await?;
                let body = serde_json::from_slice(&bytes).map_err(|_| GitHubError::schema())?;
                Ok(UsageReply { body: Some(body), cache })
            } => result,
        }
    }
    async fn validate(
        &self,
        token: &str,
        cancel: &mut watch::Receiver<u64>,
    ) -> Result<ValidatedUsage, GitHubError> {
        let raw = self
            .request(
                self.client()?
                    .get("https://api.github.com/user")
                    .bearer_auth(token),
                cancel,
            )
            .await?;
        let account: Account = serde_json::from_value(raw).map_err(|_| GitHubError::schema())?;
        if account.id == 0 || account.login.is_empty() {
            return Err(GitHubError::schema());
        }
        let reply = self
            .request_reply(
                self.client()?
                    .get("https://api.github.com/copilot_internal/user")
                    .bearer_auth(token),
                cancel,
                true,
            )
            .await?;
        validate_usage_reply(reply, account, epoch_millis())
    }
    async fn status(&self) -> AuthStatus {
        // Serialize concurrent startup status calls until credential validation has completed.
        let _restore = self.restore.lock().await;
        let should_restore = {
            let mut inner = self.inner.lock().await;
            if inner.loaded {
                false
            } else {
                match self.credentials.load() {
                    Ok(token) => {
                        inner.loaded = true;
                        inner.error = None;
                        if let Some(account) =
                            token.as_ref().and_then(|token| token.account.clone())
                        {
                            inner.snapshot = self
                                .storage
                                .latest(&account_key(&account))
                                .ok()
                                .flatten()
                                .and_then(|value| serde_json::from_value(value).ok())
                                .filter(|snapshot: &CopilotSnapshot| {
                                    snapshot.account_id == account.id
                                });
                            inner.account = Some(account);
                        }
                        inner.token = token;
                    }
                    Err(error) => inner.error = Some(error),
                }
                inner.token.is_some()
            }
        };
        if should_restore {
            let _ = self.refresh().await;
        }
        self.inner.lock().await.status()
    }
    async fn begin(&self) -> Result<DeviceAuthorization, GitHubError> {
        let (generation, mut cancel) = self.reserve().await?;
        {
            let mut inner = self.inner.lock().await;
            if inner.generation != generation {
                return Err(GitHubError::cancelled());
            }
            if inner
                .device
                .as_ref()
                .is_some_and(|d| d.expires > Instant::now())
            {
                inner.busy = false;
                return Err(GitHubError::new(
                    "busy",
                    "A GitHub sign-in is already active. Cancel it first.",
                ));
            }
            inner.device = None;
        }
        let result = async {
            let raw = self
                .request(
                    self.client()?
                        .post("https://github.com/login/device/code")
                        .form(&[("client_id", self.client_id.as_str())]),
                    &mut cancel,
                )
                .await?;
            if let Some(code) = raw.get("error").and_then(|v| v.as_str()) {
                return Err(oauth_error(code));
            }
            let response: DeviceResponse =
                serde_json::from_value(raw).map_err(|_| GitHubError::schema())?;
            if response.device_code.is_empty()
                || response.user_code.is_empty()
                || response.verification_uri != "https://github.com/login/device"
                || response.expires_in == 0
                || response.expires_in > 3600
                || response.interval.is_some_and(|v| v == 0 || v > 3600)
            {
                return Err(GitHubError::schema());
            }
            Ok(response)
        }
        .await;
        let mut inner = self.inner.lock().await;
        if inner.generation != generation {
            return Err(GitHubError::cancelled());
        }
        inner.busy = false;
        match result {
            Ok(response) => Ok(Self::activate_device(&mut inner, response)),
            Err(error) => {
                inner.authentication_error(error.clone());
                Err(error)
            }
        }
    }
    fn activate_device(inner: &mut Inner, response: DeviceResponse) -> DeviceAuthorization {
        let interval = response.interval.unwrap_or(5);
        let now = Instant::now();
        inner.device = Some(DeviceSession {
            code: response.device_code,
            expires: now + Duration::from_secs(response.expires_in),
            interval,
            next_poll: now + Duration::from_secs(interval),
        });
        if inner.token.is_none() {
            inner.error = None;
        }
        DeviceAuthorization {
            user_code: response.user_code,
            verification_uri: response.verification_uri,
            expires_in: response.expires_in,
            interval,
        }
    }
    async fn poll(&self) -> Result<PollResult, GitHubError> {
        let (generation, mut cancel) = self.reserve().await?;
        let code = {
            let mut inner = self.inner.lock().await;
            if inner.generation != generation {
                return Err(GitHubError::cancelled());
            }
            let now = Instant::now();
            match inner.device.as_ref() {
                None => {
                    inner.busy = false;
                    return Err(GitHubError::new(
                        "no_device_flow",
                        "Start GitHub sign-in first.",
                    ));
                }
                Some(device) if device.expires <= now => {
                    inner.device = None;
                    inner.busy = false;
                    return Err(oauth_error("expired_token"));
                }
                Some(device) if device.next_poll > now => {
                    let seconds = seconds_until(device.next_poll, now);
                    inner.busy = false;
                    return Ok(PollResult::pending(seconds));
                }
                Some(device) => device.code.clone(),
            }
        };
        let result = self
            .request(
                self.client()?
                    .post("https://github.com/login/oauth/access_token")
                    .form(&[
                        ("client_id", self.client_id.as_str()),
                        ("device_code", code.as_str()),
                        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ]),
                &mut cancel,
            )
            .await;
        let raw = {
            let mut inner = self.inner.lock().await;
            if generation != inner.generation {
                return Err(GitHubError::cancelled());
            }
            if inner
                .device
                .as_ref()
                .is_none_or(|d| d.expires <= Instant::now())
            {
                inner.device = None;
                inner.busy = false;
                return Err(oauth_error("expired_token"));
            }
            match result {
                Err(mut error) => {
                    inner.busy = false;
                    if let Some(device) = inner.device.as_mut() {
                        let delay = error
                            .retry_after
                            .unwrap_or(device.interval)
                            .max(device.interval);
                        device.next_poll = deadline_after(delay);
                        error.retry_after = Some(delay);
                    }
                    inner.authentication_error(error.clone());
                    return Err(error);
                }
                Ok(raw) => raw,
            }
        };
        if let Some(code) = raw.get("error").and_then(|v| v.as_str()) {
            let mut inner = self.inner.lock().await;
            if generation != inner.generation {
                return Err(GitHubError::cancelled());
            }
            inner.busy = false;
            if code == "authorization_pending" || code == "slow_down" {
                let device = inner.device.as_mut().ok_or_else(GitHubError::cancelled)?;
                if code == "slow_down" {
                    device.interval = device.interval.saturating_add(5);
                }
                device.next_poll = Instant::now() + Duration::from_secs(device.interval);
                return Ok(PollResult::pending(device.interval));
            }
            inner.device = None;
            let error = oauth_error(code);
            inner.authentication_error(error.clone());
            return Err(error);
        }
        let started_at = epoch_millis();
        let collection_id = uuid::Uuid::new_v4().to_string();
        let result = async {
            let mut token = parse_token(&raw)?;
            let usage = self
                .validate(&token.access_token, &mut cancel)
                .await
                .map_err(post_exchange_error)?;
            token.account = Some(usage.account.clone());
            Ok::<_, GitHubError>((token, usage))
        }
        .await;
        let mut inner = self.inner.lock().await;
        if generation != inner.generation {
            return Err(GitHubError::cancelled());
        }
        inner.busy = false;
        inner.device = None;
        let (token, usage) = match result {
            Ok(values) => values,
            Err(error) => {
                inner.authentication_error(error.clone());
                return Err(error);
            }
        };
        self.complete_sign_in(&mut inner, &collection_id, started_at, token, usage)
    }
    fn complete_sign_in(
        &self,
        inner: &mut Inner,
        collection_id: &str,
        started_at: u64,
        token: StoredToken,
        usage: ValidatedUsage,
    ) -> Result<PollResult, GitHubError> {
        // Serialize credential mutation with disconnect so late responses cannot restore a deleted token.
        if let Err(error) = self.credentials.save(&token) {
            inner.authentication_error(error.clone());
            return Err(error);
        }
        inner.token = Some(token);
        inner.loaded = true;
        self.apply_usage(inner, collection_id, started_at, usage, None);
        Ok(PollResult {
            state: "connected".into(),
            retry_after: None,
            status: Some(inner.status()),
        })
    }
    async fn refresh(&self) -> Result<CopilotSnapshot, GitHubError> {
        let (generation, mut cancel) = self.reserve().await?;
        let token = {
            let mut inner = self.inner.lock().await;
            if inner.generation != generation {
                return Err(GitHubError::cancelled());
            }
            if let Some(next) = inner.next_refresh.filter(|next| *next > Instant::now()) {
                inner.busy = false;
                let mut error =
                    GitHubError::new("cooldown", "Wait before refreshing GitHub usage again.");
                error.retry_after = Some(seconds_until(next, Instant::now()));
                return Err(error);
            }
            if inner
                .error
                .as_ref()
                .is_some_and(|error| error.code == "reauth_required")
            {
                inner.busy = false;
                return Err(GitHubError::new(
                    "reauth_required",
                    "Reconnect GitHub to continue.",
                ));
            }
            match inner.token.as_ref() {
                Some(token)
                    if token
                        .expires_at
                        .is_some_and(|deadline| epoch_millis() >= deadline) =>
                {
                    let error = GitHubError::new(
                        "reauth_required",
                        "GitHub authorization expired. Reconnect GitHub.",
                    );
                    inner.busy = false;
                    inner.error = Some(error.clone());
                    return Err(error);
                }
                Some(token) => token.clone(),
                None => {
                    inner.busy = false;
                    return Err(GitHubError::new("not_connected", "Connect GitHub first."));
                }
            }
        };
        let started_at = epoch_millis();
        let collection_id = uuid::Uuid::new_v4().to_string();
        let result = self.validate(&token.access_token, &mut cancel).await;
        let mut inner = self.inner.lock().await;
        if generation != inner.generation {
            return Err(GitHubError::cancelled());
        }
        inner.busy = false;
        self.finish_refresh(&mut inner, &collection_id, started_at, &token, result)
    }
    fn finish_refresh(
        &self,
        inner: &mut Inner,
        collection_id: &str,
        started_at: u64,
        token: &StoredToken,
        result: Result<ValidatedUsage, GitHubError>,
    ) -> Result<CopilotSnapshot, GitHubError> {
        // Identity failures follow the same collection outcome and scheduling
        // path as HTTP/schema failures; otherwise each background tick retries.
        let result = result.and_then(|usage| {
            if inner
                .account
                .as_ref()
                .is_some_and(|old| old.id != usage.account.id)
            {
                Err(GitHubError::new(
                    "identity_mismatch",
                    "GitHub identity changed. Reconnect before collecting usage.",
                ))
            } else {
                Ok(usage)
            }
        });
        match result {
            Ok(usage) => {
                let account = &usage.account;
                // Upgrade legacy credentials with verified identity for account-scoped offline restore.
                let credential_error = if token
                    .account
                    .as_ref()
                    .is_none_or(|saved| saved.id != account.id)
                {
                    let updated = StoredToken {
                        account: Some(account.clone()),
                        ..token.clone()
                    };
                    self.credentials.save(&updated).err()
                } else {
                    None
                };
                // Reflect only metadata that reached the credential store. A
                // failed upgrade must remain eligible for the next fresh poll.
                if credential_error.is_none() {
                    if let Some(saved_token) = inner.token.as_mut() {
                        saved_token.account = Some(account.clone());
                    }
                }
                self.apply_usage(inner, collection_id, started_at, usage, credential_error)
                    .ok_or_else(cached_response_error)
            }
            Err(mut error) => {
                if let Some(account) = &inner.account {
                    let _ = self.storage.record_failure(
                        collection_id,
                        &account_key(account),
                        started_at,
                        epoch_millis(),
                        &error.code,
                    );
                }
                inner.failures = inner.failures.saturating_add(1);
                let delay = refresh_retry_delay(inner.failures, error.retry_after);
                inner.next_refresh = Some(deadline_after(delay));
                error.retry_after = Some(delay);
                inner.error = Some(error.clone());
                Err(error)
            }
        }
    }
    fn apply_usage(
        &self,
        inner: &mut Inner,
        collection_id: &str,
        started_at: u64,
        usage: ValidatedUsage,
        credential_error: Option<GitHubError>,
    ) -> Option<CopilotSnapshot> {
        // Keep cache ownership safe for every caller, including cached-only account switches.
        if inner
            .account
            .as_ref()
            .is_none_or(|previous| previous.id != usage.account.id)
            || inner
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.account_id != usage.account.id)
        {
            inner.snapshot = None;
        }
        let snapshot = usage.snapshot;
        inner.error = if let Some(snapshot) = &snapshot {
            let error = self
                .persist(collection_id, started_at, &usage.account, snapshot)
                .err();
            inner.snapshot = Some(snapshot.clone());
            error.or(credential_error)
        } else {
            Some(credential_error.unwrap_or_else(cached_response_error))
        };
        inner.account = Some(usage.account);
        inner.next_refresh = Some(deadline_after(usage.cache_delay.max(30)));
        inner.last_successful_poll = Some(Instant::now());
        inner.failures = 0;
        snapshot
    }
    async fn cancel(&self) {
        let mut inner = self.inner.lock().await;
        inner.generation = inner.generation.wrapping_add(1);
        self.cancel.send_replace(inner.generation);
        inner.device = None;
        inner.busy = false;
    }
    async fn disconnect(&self) -> Result<AuthStatus, GitHubError> {
        let mut inner = self.inner.lock().await;
        inner.generation = inner.generation.wrapping_add(1);
        self.cancel.send_replace(inner.generation);
        inner.device = None;
        inner.busy = false;
        inner.token = None;
        inner.account = None;
        inner.snapshot = None;
        inner.loaded = true;
        inner.next_refresh = None;
        inner.last_successful_poll = None;
        inner.failures = 0;
        let deletion = self.credentials.delete();
        match deletion {
            Ok(()) => {
                inner.error = None;
                Ok(inner.status())
            }
            Err(error) => {
                inner.error = Some(error.clone());
                Err(error)
            }
        }
    }
}
#[derive(Default)]
struct CachePolicy {
    delay: u64,
    from_cache: bool,
}
struct UsageReply {
    body: Option<serde_json::Value>,
    cache: CachePolicy,
}
struct ValidatedUsage {
    account: Account,
    snapshot: Option<CopilotSnapshot>,
    cache_delay: u64,
}
fn validate_usage_reply(
    reply: UsageReply,
    account: Account,
    fetched_at: u64,
) -> Result<ValidatedUsage, GitHubError> {
    let snapshot = match reply.body {
        Some(raw) => {
            let snapshot = project_copilot(raw, &account, fetched_at)?;
            if snapshot.premium.credits_used.is_none() {
                return Err(GitHubError::new(
                    "metric_unavailable",
                    "GitHub did not report premium usage. Last valid usage is retained.",
                ));
            }
            // A positive Age explicitly identifies a reused response, not a fresh poll.
            (!reply.cache.from_cache).then_some(snapshot)
        }
        None => None, // A 304 cannot create a fresh observation or fetched_at.
    };
    Ok(ValidatedUsage {
        account,
        snapshot,
        cache_delay: reply.cache.delay,
    })
}
fn cached_response_error() -> GitHubError {
    GitHubError::new(
        "cached_response",
        "GitHub returned cached usage. Last live observation is retained.",
    )
}
fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
}
fn decimal_seconds(value: &str) -> Option<u64> {
    let value = value.trim();
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}
fn system_seconds_until(deadline: SystemTime, now: SystemTime) -> u64 {
    let duration = deadline.duration_since(now).unwrap_or_default();
    duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() != 0))
}
fn retry_after_seconds(value: &str, now: SystemTime) -> Option<u64> {
    decimal_seconds(value).or_else(|| {
        httpdate::parse_http_date(value)
            .ok()
            .map(|deadline| system_seconds_until(deadline, now))
    })
}
fn deadline_after(seconds: u64) -> Instant {
    let now = Instant::now();
    // Never convert an unrepresentable provider delay into an absent cooldown.
    // One hundred years is a fail-closed bound beyond the useful token lifetime.
    now.checked_add(Duration::from_secs(seconds))
        .unwrap_or_else(|| now + Duration::from_secs(100 * 366 * 86_400))
}
fn deadline_epoch_millis(deadline: Instant, now: Instant, utc_now: u64) -> u64 {
    let remaining = deadline.saturating_duration_since(now);
    let millis = remaining.as_millis().saturating_add(u128::from(
        !remaining.subsec_nanos().is_multiple_of(1_000_000),
    ));
    utc_now.saturating_add(millis.min(u64::MAX as u128) as u64)
}
fn cache_policy(headers: &HeaderMap, now: SystemTime) -> CachePolicy {
    let age = header_text(headers, "age").and_then(decimal_seconds);
    let mut policy = CachePolicy {
        delay: 0,
        from_cache: age.is_some_and(|age| age > 0),
    };
    let mut max_age = None;
    let mut forbidden = false;
    for value in headers.get_all("cache-control") {
        let Ok(value) = value.to_str() else {
            return policy;
        };
        for directive in value.split(',').map(str::trim) {
            let (name, value) = directive
                .split_once('=')
                .map(|(name, value)| (name.trim(), Some(value.trim())))
                .unwrap_or((directive, None));
            if name.eq_ignore_ascii_case("no-cache") || name.eq_ignore_ascii_case("no-store") {
                forbidden = true;
            }
            if name.eq_ignore_ascii_case("max-age") {
                // Conflicting/duplicate freshness directives are not usable guidance.
                if max_age.is_some() {
                    return policy;
                }
                let Some(value) = value else {
                    return policy;
                };
                let value = value
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .unwrap_or(value);
                let Some(value) = decimal_seconds(value) else {
                    return policy;
                };
                max_age = Some(value);
            }
        }
    }
    if !forbidden {
        if let Some(max_age) = max_age {
            // Date can reveal additional apparent age; never wait past known freshness.
            let apparent_age = header_text(headers, "date")
                .and_then(|value| httpdate::parse_http_date(value).ok())
                .map(|date| now.duration_since(date).unwrap_or_default().as_secs())
                .unwrap_or(0);
            policy.delay = max_age.saturating_sub(age.unwrap_or(0).max(apparent_age));
        }
    }
    policy
}

fn refresh_retry_delay(failures: u32, provider_delay: Option<u64>) -> u64 {
    // Brief network interruptions get three quick retries before backing off.
    // This schedule is independent of the normal usage polling interval.
    let delay = match failures {
        0..=3 => 5,
        4 => 15,
        5 => 30,
        6 => 60,
        7 => 120,
        _ => 300,
    };
    provider_delay.unwrap_or(0).max(delay)
}
fn history_error() -> GitHubError {
    GitHubError::new(
        "history_storage",
        "Usage refreshed, but local history could not be saved.",
    )
}
fn account_key(account: &Account) -> AccountKey {
    AccountKey {
        provider: "github_copilot".into(),
        host: "api.github.com".into(),
        account_id: account.id.to_string(),
    }
}
async fn bounded_body(mut response: Response) -> Result<Vec<u8>, GitHubError> {
    const MAX_RESPONSE: usize = 1024 * 1024;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE as u64)
    {
        return Err(GitHubError::schema());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| GitHubError::new("network", "GitHub response was interrupted."))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err(GitHubError::schema());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn classify_http(status: u16, remaining: Option<&str>, retry_after: Option<u64>) -> GitHubError {
    let rate_limited =
        status == 429 || (status == 403 && (remaining == Some("0") || retry_after.is_some()));
    let mut error = if rate_limited {
        GitHubError::new("rate_limited", "GitHub rate limit reached. Retry later.")
    } else {
        match status {
            401 => GitHubError::new(
                "reauth_required",
                "GitHub authorization expired or was revoked. Reconnect GitHub.",
            ),
            403 => GitHubError::new(
                "permission_denied",
                "GitHub denied access. Check account permissions or organization policy.",
            ),
            404 => GitHubError::new(
                "endpoint_unavailable",
                "GitHub endpoint unavailable for this account.",
            ),
            _ => GitHubError::new("http", "GitHub returned an unsuccessful response."),
        }
    };
    error.retry_after = retry_after;
    error
}
fn http_error(response: &Response) -> GitHubError {
    http_error_headers(
        response.status().as_u16(),
        response.headers(),
        SystemTime::now(),
    )
}
fn http_error_headers(status: u16, headers: &HeaderMap, now: SystemTime) -> GitHubError {
    let retry_after =
        header_text(headers, "retry-after").and_then(|value| retry_after_seconds(value, now));
    let mut error = classify_http(
        status,
        header_text(headers, "x-ratelimit-remaining"),
        retry_after,
    );
    if error.code == "rate_limited" {
        let reset = header_text(headers, "x-ratelimit-reset")
            .and_then(decimal_seconds)
            .and_then(|seconds| UNIX_EPOCH.checked_add(Duration::from_secs(seconds)))
            .map(|deadline| system_seconds_until(deadline, now));
        error.retry_after = match (error.retry_after, reset) {
            (Some(retry), Some(reset)) => Some(retry.max(reset)),
            (retry, reset) => retry.or(reset),
        };
    }
    error
}
// A successful exchange consumes the device code. Transient validation failures
// cannot retry that exchange: the temporary token is discarded and sign-in must restart.
fn post_exchange_error(error: GitHubError) -> GitHubError {
    match error.code.as_str() {
        "network" | "rate_limited" | "http" => GitHubError {
            code: "validation_failed".into(),
            message: "GitHub issued authorization, but account and Copilot access could not be verified. The temporary credential was discarded. Start sign-in again.".into(),
            retry_after: error.retry_after,
        },
        _ => error,
    }
}
fn oauth_error(code: &str) -> GitHubError {
    match code {
        "access_denied" => GitHubError::new("authorization_denied", "GitHub sign-in was declined."),
        "expired_token" => GitHubError::new(
            "device_expired",
            "GitHub sign-in code expired. Start again.",
        ),
        "incorrect_client_credentials" | "device_flow_disabled" => GitHubError::new(
            "configuration",
            "Check Luma's client ID and enable Device Flow in the GitHub OAuth application.",
        ),
        _ => GitHubError::new("oauth", "GitHub could not complete device authorization."),
    }
}
fn parse_token(raw: &serde_json::Value) -> Result<StoredToken, GitHubError> {
    // Extra lifecycle fields do not invalidate a usable OAuth access token.
    // Reauthentication on HTTP 401 remains the supported expiry/revocation path.
    // Refresh tokens are deliberately neither persisted nor used.
    if !raw
        .get("token_type")
        .and_then(|v| v.as_str())
        .is_some_and(|v| v.eq_ignore_ascii_case("bearer"))
    {
        return Err(GitHubError::schema());
    }
    let scope = raw
        .get("scope")
        .and_then(|v| v.as_str())
        .ok_or_else(GitHubError::schema)?;
    if !scope.trim().is_empty() {
        return Err(GitHubError::new(
            "unexpected_scope",
            "GitHub returned permissions beyond Luma's validated empty scope request.",
        ));
    }
    let access_token = raw
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .ok_or_else(GitHubError::schema)?
        .to_owned();
    Ok(StoredToken {
        access_token,
        expires_at: token_expiration(raw, epoch_millis())?,
        account: None,
    })
}
fn project_copilot(
    raw: serde_json::Value,
    account: &Account,
    fetched_at: u64,
) -> Result<CopilotSnapshot, GitHubError> {
    #[derive(Deserialize)]
    struct Quotas {
        premium_interactions: PremiumQuota,
    }
    #[derive(Deserialize)]
    struct Raw {
        login: String,
        copilot_plan: Option<String>,
        quota_reset_date_utc: Option<String>,
        quota_snapshots: Quotas,
    }
    let raw: Raw = serde_json::from_value(raw).map_err(|_| GitHubError::schema())?;
    if !raw.login.eq_ignore_ascii_case(&account.login) {
        return Err(GitHubError::new(
            "identity_mismatch",
            "Copilot usage did not match the authenticated GitHub account.",
        ));
    }
    let premium = raw.quota_snapshots.premium_interactions;
    if premium
        .entitlement
        .as_ref()
        .and_then(|n| n.as_f64())
        .is_some_and(|v| v < 0.0)
        || premium
            .credits_used
            .as_ref()
            .and_then(|n| n.as_f64())
            .is_some_and(|v| v < 0.0)
    {
        return Err(GitHubError::schema());
    }
    // JSON numbers retain their exact decimal representation for persistence.
    // Floating-point conversion is used only for the presentation percentage.
    let used_percent = match (
        premium.credits_used.as_ref().and_then(|n| n.as_f64()),
        premium.entitlement.as_ref().and_then(|n| n.as_f64()),
    ) {
        (Some(used), Some(limit)) if limit > 0.0 && premium.unlimited != Some(true) => {
            let percent = used / limit * 100.0;
            percent.is_finite().then_some(percent)
        }
        _ => None,
    };
    Ok(CopilotSnapshot {
        account_id: account.id,
        login: raw.login,
        plan: raw.copilot_plan,
        reset_at: raw.quota_reset_date_utc,
        fetched_at,
        premium,
        used_percent,
    })
}
#[tauri::command]
pub async fn github_status(service: State<'_, GitHubService>) -> Result<AuthStatus, GitHubError> {
    Ok(service.status().await)
}
#[tauri::command]
pub async fn github_begin(
    service: State<'_, GitHubService>,
) -> Result<DeviceAuthorization, GitHubError> {
    service.begin().await
}
#[tauri::command]
pub async fn github_poll(
    app: tauri::AppHandle,
    service: State<'_, GitHubService>,
) -> Result<PollResult, GitHubError> {
    let result = service.poll().await;
    if result.as_ref().is_ok_and(|poll| poll.state == "connected") {
        let _ = app.emit("github-status", service.inner.lock().await.status());
    }
    result
}
#[tauri::command]
pub async fn github_cancel(service: State<'_, GitHubService>) -> Result<(), GitHubError> {
    service.cancel().await;
    Ok(())
}
#[tauri::command]
pub async fn github_disconnect(
    app: tauri::AppHandle,
    service: State<'_, GitHubService>,
) -> Result<AuthStatus, GitHubError> {
    let result = service.disconnect().await;
    let _ = app.emit("github-status", service.inner.lock().await.status());
    result
}
#[tauri::command]
pub async fn github_refresh(
    app: tauri::AppHandle,
    service: State<'_, GitHubService>,
) -> Result<CopilotSnapshot, GitHubError> {
    let result = service.refresh().await;
    let _ = app.emit("github-status", service.inner.lock().await.status());
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn account() -> Account {
        Account {
            id: 42,
            login: "yew".into(),
        }
    }
    fn fixture() -> serde_json::Value {
        json!({"login":"yew","copilot_plan":"enterprise","quota_reset_date_utc":"2026-10-01T00:00:00.000Z",
            "quota_snapshots":{"premium_interactions":{"entitlement":2000000,"credits_used":722342,
            "quota_remaining":1277551.1,"percent_remaining":63.8,"overage_permitted":true}}})
    }
    #[derive(Default)]
    struct FakeCredentials {
        state: std::sync::Mutex<FakeCredentialState>,
    }
    #[derive(Default)]
    struct FakeCredentialState {
        token: Option<StoredToken>,
        fail_load: bool,
        fail_save: bool,
        fail_delete: bool,
        loads: usize,
        saves: usize,
        deletes: usize,
    }
    impl CredentialStore for FakeCredentials {
        fn load(&self) -> Result<Option<StoredToken>, GitHubError> {
            let mut state = self.state.lock().unwrap();
            state.loads += 1;
            if state.fail_load {
                Err(GitHubError::storage())
            } else {
                Ok(state.token.clone())
            }
        }
        fn save(&self, token: &StoredToken) -> Result<(), GitHubError> {
            let mut state = self.state.lock().unwrap();
            state.saves += 1;
            if state.fail_save {
                Err(GitHubError::storage())
            } else {
                state.token = Some(token.clone());
                Ok(())
            }
        }
        fn delete(&self) -> Result<(), GitHubError> {
            let mut state = self.state.lock().unwrap();
            state.deletes += 1;
            if state.fail_delete {
                Err(GitHubError::storage())
            } else {
                state.token = None;
                Ok(())
            }
        }
    }
    fn fake_service() -> (GitHubService, Arc<FakeCredentials>) {
        let credentials = Arc::new(FakeCredentials::default());
        let service = GitHubService::with_credentials(
            Arc::new(Storage::in_memory().unwrap()),
            &crate::network::ProxySettings {
                mode: crate::network::ProxyMode::Direct,
                server: String::new(),
            },
            credentials.clone(),
        )
        .unwrap();
        (service, credentials)
    }
    fn test_token(account: Account) -> StoredToken {
        StoredToken {
            access_token: "test-only-never-sent".into(),
            expires_at: None,
            account: Some(account),
        }
    }
    fn test_device() -> DeviceResponse {
        DeviceResponse {
            device_code: "test-device-never-sent".into(),
            user_code: "TEST-CODE".into(),
            verification_uri: "https://github.com/login/device".into(),
            expires_in: 60,
            interval: Some(5),
        }
    }
    fn test_usage(account: Account, fetched_at: u64) -> ValidatedUsage {
        let mut raw = fixture();
        raw["login"] = json!(account.login);
        ValidatedUsage {
            snapshot: Some(project_copilot(raw, &account, fetched_at).unwrap()),
            account,
            cache_delay: 0,
        }
    }
    #[tokio::test]
    async fn locked_credential_store_can_be_retried_without_restart_or_network() {
        let (service, credentials) = fake_service();
        let mut expired = test_token(account());
        expired.expires_at = Some(1); // Any accidental request is blocked locally.
        {
            let mut state = credentials.state.lock().unwrap();
            state.token = Some(expired);
            state.fail_load = true;
        }
        let unavailable = service.status().await;
        assert_eq!(unavailable.state, "error");
        assert_eq!(unavailable.error.unwrap().code, "secure_storage");
        assert!(!service.inner.lock().await.loaded);
        credentials.state.lock().unwrap().fail_load = false;
        let restored = service.status().await;
        assert_eq!(restored.state, "reauth_required");
        assert_eq!(restored.account.unwrap().id, account().id);
        assert_eq!(credentials.state.lock().unwrap().loads, 2);
        assert!(service.inner.lock().await.loaded);
        assert_eq!(service.status().await.state, "reauth_required");
        assert_eq!(credentials.state.lock().unwrap().loads, 2);
    }
    #[tokio::test]
    async fn retrying_unlocked_empty_store_clears_old_storage_error() {
        let (service, credentials) = fake_service();
        credentials.state.lock().unwrap().fail_load = true;
        assert_eq!(service.status().await.state, "error");
        credentials.state.lock().unwrap().fail_load = false;
        let recovered = service.status().await;
        assert_eq!(recovered.state, "disconnected");
        assert!(recovered.error.is_none());
        assert!(service.inner.lock().await.loaded);
    }
    #[tokio::test]
    async fn cancelled_or_failed_reconnect_preserves_revoked_connection_and_snapshot() {
        let (service, _) = fake_service();
        {
            let mut inner = service.inner.lock().await;
            inner.loaded = true;
            inner.token = Some(test_token(account()));
            inner.account = Some(account());
            inner.snapshot = test_usage(account(), 100).snapshot;
            inner.error = Some(GitHubError::new("reauth_required", "Reconnect GitHub."));
            GitHubService::activate_device(&mut inner, test_device());
            assert_eq!(inner.status().state, "reauth_required");
            for code in [
                "authorization_denied",
                "network",
                "permission_denied",
                "secure_storage",
            ] {
                inner.authentication_error(GitHubError::new(code, "Replacement sign-in failed."));
                assert_eq!(inner.status().state, "reauth_required");
                assert_eq!(inner.snapshot.as_ref().unwrap().fetched_at, 100);
            }
        }
        service.cancel().await;
        assert_eq!(service.status().await.state, "reauth_required");
        assert_eq!(service.refresh().await.unwrap_err().code, "reauth_required");
        assert!(service.inner.lock().await.device.is_none());
    }
    #[tokio::test]
    async fn secure_store_save_failure_never_commits_replacement_identity_or_usage() {
        let (service, credentials) = fake_service();
        let original_token = test_token(account());
        credentials.state.lock().unwrap().token = Some(original_token.clone());
        credentials.state.lock().unwrap().fail_save = true;
        let replacement = Account {
            id: 84,
            login: "another".into(),
        };
        let now = epoch_millis();
        let mut inner = service.inner.lock().await;
        inner.token = Some(original_token);
        inner.account = Some(account());
        inner.snapshot = test_usage(account(), now).snapshot;
        inner.error = Some(GitHubError::new("reauth_required", "Reconnect GitHub."));
        let result = service.complete_sign_in(
            &mut inner,
            "replacement-failed",
            now,
            test_token(replacement.clone()),
            test_usage(replacement.clone(), now),
        );
        assert_eq!(result.err().unwrap().code, "secure_storage");
        assert_eq!(inner.status().state, "reauth_required");
        assert_eq!(inner.account.as_ref().unwrap().id, account().id);
        assert_eq!(inner.snapshot.as_ref().unwrap().account_id, account().id);
        assert!(service
            .storage
            .latest(&account_key(&replacement))
            .unwrap()
            .is_none());
        assert_eq!(
            credentials
                .state
                .lock()
                .unwrap()
                .token
                .as_ref()
                .unwrap()
                .account
                .as_ref()
                .unwrap()
                .id,
            account().id
        );
        // An initial connection cannot claim success either.
        inner.token = None;
        inner.account = None;
        inner.snapshot = None;
        inner.error = None;
        let result = service.complete_sign_in(
            &mut inner,
            "initial-failed",
            now,
            test_token(replacement.clone()),
            test_usage(replacement, now),
        );
        assert_eq!(result.err().unwrap().code, "secure_storage");
        assert_eq!(inner.status().state, "error");
        assert!(inner.token.is_none());
        assert!(inner.snapshot.is_none());
    }
    #[tokio::test]
    async fn disconnect_store_failure_stays_disconnected_and_deletion_can_be_retried() {
        let (service, credentials) = fake_service();
        {
            let mut state = credentials.state.lock().unwrap();
            state.token = Some(test_token(account()));
            state.fail_delete = true;
        }
        {
            let mut inner = service.inner.lock().await;
            inner.loaded = true;
            inner.token = Some(test_token(account()));
            inner.account = Some(account());
            inner.snapshot = test_usage(account(), 100).snapshot;
            GitHubService::activate_device(&mut inner, test_device());
        }
        let (_, mut cancellation) = service.reserve().await.unwrap();
        assert_eq!(
            service.disconnect().await.err().unwrap().code,
            "secure_storage"
        );
        assert!(cancellation.changed().await.is_ok());
        let status = service.status().await;
        assert_eq!(status.state, "error");
        assert!(status.account.is_none());
        assert!(status.snapshot.is_none());
        assert_eq!(credentials.state.lock().unwrap().loads, 0);
        assert!(!service.inner.lock().await.busy);
        credentials.state.lock().unwrap().fail_delete = false;
        assert_eq!(service.disconnect().await.unwrap().state, "disconnected");
        assert!(credentials.state.lock().unwrap().token.is_none());
        assert_eq!(credentials.state.lock().unwrap().deletes, 2);
    }
    #[tokio::test]
    async fn changed_refresh_identity_uses_failure_backoff_and_keeps_last_usage() {
        let (service, credentials) = fake_service();
        let original_token = test_token(account());
        let replacement = Account {
            id: 84,
            login: "another".into(),
        };
        let now = epoch_millis();
        let mut inner = service.inner.lock().await;
        inner.token = Some(original_token.clone());
        inner.account = Some(account());
        inner.snapshot = test_usage(account(), now).snapshot;
        for (id, delay) in [("identity-first", 5), ("identity-second", 5)] {
            let error = service
                .finish_refresh(
                    &mut inner,
                    id,
                    now,
                    &original_token,
                    Ok(test_usage(replacement.clone(), now)),
                )
                .unwrap_err();
            assert_eq!(error.code, "identity_mismatch");
            assert_eq!(error.retry_after, Some(delay));
            assert!(
                inner
                    .next_refresh
                    .unwrap()
                    .duration_since(Instant::now())
                    .as_secs()
                    >= delay - 1
            );
            assert_eq!(inner.account.as_ref().unwrap().id, account().id);
            assert_eq!(inner.snapshot.as_ref().unwrap().account_id, account().id);
        }
        assert_eq!(inner.failures, 2);
        assert_eq!(credentials.state.lock().unwrap().saves, 0);
        assert!(service
            .storage
            .latest(&account_key(&replacement))
            .unwrap()
            .is_none());
        drop(inner);
        assert_eq!(service.refresh().await.unwrap_err().code, "cooldown");
    }

    #[tokio::test]
    async fn failed_legacy_identity_upgrade_keeps_usage_and_retries_secure_storage() {
        let (service, credentials) = fake_service();
        let legacy = StoredToken {
            account: None,
            ..test_token(account())
        };
        {
            let mut state = credentials.state.lock().unwrap();
            state.token = Some(legacy.clone());
            state.fail_save = true;
        }
        let mut inner = service.inner.lock().await;
        inner.token = Some(legacy.clone());
        let now = epoch_millis();
        let first = service
            .finish_refresh(
                &mut inner,
                "legacy-first",
                now,
                &legacy,
                Ok(test_usage(account(), now)),
            )
            .unwrap();
        assert_eq!(first.account_id, account().id);
        assert_eq!(inner.error.as_ref().unwrap().code, "secure_storage");
        assert!(inner.token.as_ref().unwrap().account.is_none());
        assert_eq!(inner.account.as_ref().unwrap().id, account().id);
        assert!(service
            .storage
            .latest(&account_key(&account()))
            .unwrap()
            .is_some());
        credentials.state.lock().unwrap().fail_save = false;
        let retry_token = inner.token.clone().unwrap();
        let next = now + 1;
        service
            .finish_refresh(
                &mut inner,
                "legacy-retry",
                next,
                &retry_token,
                Ok(test_usage(account(), next)),
            )
            .unwrap();
        assert!(inner.error.is_none());
        assert_eq!(inner.snapshot.as_ref().unwrap().fetched_at, next);
        assert_eq!(
            inner.token.as_ref().unwrap().account.as_ref().unwrap().id,
            account().id
        );
        let state = credentials.state.lock().unwrap();
        assert_eq!(state.saves, 2);
        assert_eq!(
            state.token.as_ref().unwrap().account.as_ref().unwrap().id,
            account().id
        );
    }

    #[tokio::test]
    async fn proxy_replacement_applies_to_oauth_and_preserves_old_client_on_save_failure() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let observed = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "Updated proxy was not used");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("Test proxy: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") && bytes.len() < 8192 {
                let mut byte = [0];
                assert_eq!(stream.read(&mut byte).unwrap(), 1);
                bytes.push(byte[0]);
            }
            stream
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            String::from_utf8(bytes).unwrap()
        });
        let direct = crate::network::ProxySettings {
            mode: crate::network::ProxyMode::Direct,
            server: String::new(),
        };
        let service = GitHubService::new(Arc::new(Storage::in_memory().unwrap()), &direct).unwrap();
        let configured = crate::network::ProxySettings {
            mode: crate::network::ProxyMode::Http,
            server: format!("http://{address}"),
        };
        service
            .replace_client(
                crate::network::build_client(&configured).unwrap(),
                || Ok(()),
            )
            .unwrap();
        assert!(service
            .replace_client(crate::network::build_client(&direct).unwrap(), || Err(
                "test storage failure".into()
            ))
            .is_err());
        assert!(service.begin().await.is_err());
        assert!(observed
            .join()
            .unwrap()
            .starts_with("CONNECT github.com:443 HTTP/1.1"));
        assert!(!service.inner.lock().await.busy);
    }

    #[test]
    fn preserves_provider_values_and_calculates_independent_percentage() {
        let result = project_copilot(fixture(), &account(), 123).unwrap();
        assert!((result.used_percent.unwrap() - 36.1171).abs() < 0.000001);
        assert_eq!(
            result.premium.quota_remaining.unwrap().to_string(),
            "1277551.1"
        );
        assert_eq!(
            result.premium.percent_remaining.unwrap().to_string(),
            "63.8"
        );
        assert_eq!(result.fetched_at, 123);
        assert_eq!(result.account_id, 42);
    }
    #[test]
    fn unknown_zero_and_over_quota_are_distinct() {
        let mut raw = fixture();
        raw["quota_snapshots"]["premium_interactions"]["entitlement"] = json!(0);
        assert_eq!(
            project_copilot(raw.clone(), &account(), 0)
                .unwrap()
                .used_percent,
            None
        );
        raw["quota_snapshots"]["premium_interactions"] = json!({});
        let unknown = project_copilot(raw.clone(), &account(), 0).unwrap();
        assert_eq!(unknown.premium.credits_used, None);
        assert_eq!(unknown.used_percent, None);
        raw["quota_snapshots"]["premium_interactions"] =
            json!({"entitlement":10,"credits_used":15});
        assert_eq!(
            project_copilot(raw, &account(), 0).unwrap().used_percent,
            Some(150.0)
        );
    }
    #[test]
    fn invalid_schema_and_identity_do_not_become_zero_usage() {
        let mut raw = fixture();
        raw["login"] = json!("another-account");
        assert_eq!(
            project_copilot(raw, &account(), 0).unwrap_err().code,
            "identity_mismatch"
        );
        let mut raw = fixture();
        raw["quota_snapshots"]["premium_interactions"]["credits_used"] = json!("722342");
        assert_eq!(
            project_copilot(raw, &account(), 0).unwrap_err().code,
            "schema"
        );
        assert_eq!(
            project_copilot(json!({}), &account(), 0).unwrap_err().code,
            "schema"
        );
    }
    #[test]
    fn fresh_copilot_collections_preserve_exact_values_and_account_identity() {
        let service = GitHubService::new(
            Arc::new(Storage::in_memory().unwrap()),
            &crate::network::ProxySettings::default(),
        )
        .unwrap();
        let raw: serde_json::Value = serde_json::from_str(r#"{"login":"yew","quota_snapshots":{"premium_interactions":{"entitlement":2000000,"credits_used":722342,"quota_remaining":1277551.100000000000000000001,"percent_remaining":63.8}}}"#).unwrap();
        let snapshot = project_copilot(raw, &account(), 100).unwrap();
        service.persist("one", 90, &account(), &snapshot).unwrap();
        service.persist("one", 90, &account(), &snapshot).unwrap();
        let query = crate::storage::HistoryQuery {
            account: account_key(&account()),
            metric: "premium_interactions".into(),
            unit: "provider_quota_credit".into(),
            semantics_version: "copilot-premium-v1".into(),
            from: 0,
            to: 1000,
            limit: 10,
            cursor: None,
        };
        let page = service.storage.query(query).unwrap();
        assert_eq!(page.observations.len(), 1);
        assert_eq!(
            page.observations[0].metric.remaining.as_deref(),
            Some("1277551.100000000000000000001")
        );
        assert_eq!(page.observations[0].continuity_confidence, "unknown");
        let cached: CopilotSnapshot = serde_json::from_value(
            service
                .storage
                .latest(&account_key(&account()))
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(cached.account_id, account().id);
        assert_eq!(
            cached.premium.quota_remaining,
            snapshot.premium.quota_remaining
        );
        let different = Account {
            id: 43,
            login: "another".into(),
        };
        assert!(service
            .storage
            .latest(&account_key(&different))
            .unwrap()
            .is_none());
    }
    #[tokio::test]
    async fn cache_deletion_invalidates_inflight_collection_and_clears_both_projections() {
        let service = GitHubService::new(
            Arc::new(Storage::in_memory().unwrap()),
            &crate::network::ProxySettings::default(),
        )
        .unwrap();
        let snapshot = project_copilot(fixture(), &account(), 100).unwrap();
        service
            .persist("before-clear", 90, &account(), &snapshot)
            .unwrap();
        {
            let mut inner = service.inner.lock().await;
            inner.account = Some(account());
            inner.snapshot = Some(snapshot);
        }
        let (generation, mut cancel) = service.reserve().await.unwrap();
        let (_, status) = service
            .clear_cache_inner(Some(&account_key(&account())))
            .await
            .unwrap();
        assert!(status.snapshot.is_none());
        assert!(service
            .storage
            .latest(&account_key(&account()))
            .unwrap()
            .is_none());
        assert!(cancel.changed().await.is_ok());
        assert_ne!(generation, service.inner.lock().await.generation);
        assert!(!service.inner.lock().await.busy);
    }
    fn headers(values: &[(&'static str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in values {
            headers.append(*name, value.parse().unwrap());
        }
        headers
    }
    #[test]
    fn provider_retry_dates_and_reset_limits_use_the_longest_wait() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000) + Duration::from_millis(500);
        let future = httpdate::fmt_http_date(UNIX_EPOCH + Duration::from_secs(1_700_000_090));
        assert_eq!(retry_after_seconds(" 120 ", now), Some(120));
        assert_eq!(retry_after_seconds(&future, now), Some(90));
        assert_eq!(
            retry_after_seconds("Thu, 01 Jan 1970 00:00:00 GMT", now),
            Some(0)
        );
        for value in ["-1", "1.5", "later", "18446744073709551616"] {
            assert!(retry_after_seconds(value, now).is_none());
        }
        let error = http_error_headers(
            403,
            &headers(&[
                ("retry-after", &future),
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", "1700000200"),
            ]),
            now,
        );
        assert_eq!(error.code, "rate_limited");
        assert_eq!(error.retry_after, Some(200));
        let denied = http_error_headers(403, &headers(&[("x-ratelimit-reset", "1700000200")]), now);
        assert_eq!(denied.code, "permission_denied");
        assert_eq!(denied.retry_after, None);
        let unavailable = http_error_headers(503, &headers(&[("retry-after", &future)]), now);
        assert_eq!(unavailable.retry_after, Some(90));
    }
    #[test]
    fn cache_lifetime_subtracts_age_and_ignores_unusable_directives() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let recent = httpdate::fmt_http_date(now - Duration::from_secs(120));
        let policy = cache_policy(
            &headers(&[
                ("cache-control", "private, max-age=600"),
                ("age", "60"),
                ("date", &recent),
            ]),
            now,
        );
        assert_eq!(policy.delay, 480);
        assert!(policy.from_cache);
        let fresh = cache_policy(
            &headers(&[("cache-control", "MAX-AGE=\"600\""), ("age", "0")]),
            now,
        );
        assert_eq!(fresh.delay, 600);
        assert!(!fresh.from_cache);
        for control in [
            "max-age=600, no-store",
            "max-age=600, no-cache",
            "max-age=-1",
            "max-age=600, max-age=900",
            "max-age=oops",
            "s-maxage=600",
        ] {
            assert_eq!(
                cache_policy(&headers(&[("cache-control", control)]), now).delay,
                0
            );
        }
        assert_eq!(
            cache_policy(
                &headers(&[("cache-control", "max-age=60"), ("age", "120")]),
                now
            )
            .delay,
            0
        );
    }
    #[test]
    fn deadlines_round_up_and_never_drop_overflowed_cooldowns() {
        let now = Instant::now();
        assert_eq!(
            deadline_epoch_millis(now + Duration::from_micros(1_501), now, 100),
            102
        );
        assert_eq!(
            deadline_epoch_millis(now - Duration::from_secs(1), now, 100),
            100
        );
        assert!(deadline_after(u64::MAX) > now + Duration::from_secs(86_400));
    }
    #[tokio::test]
    async fn cache_only_replies_preserve_original_observation_and_fresh_replies_extend_cooldown() {
        let service = GitHubService::new(
            Arc::new(Storage::in_memory().unwrap()),
            &crate::network::ProxySettings::default(),
        )
        .unwrap();
        let mut inner = service.inner.lock().await;
        let fresh = validate_usage_reply(
            UsageReply {
                body: Some(fixture()),
                cache: CachePolicy {
                    delay: 600,
                    from_cache: false,
                },
            },
            account(),
            100,
        )
        .unwrap();
        service.apply_usage(&mut inner, "fresh", 90, fresh, None);
        assert!(inner.next_refresh.unwrap() >= Instant::now() + Duration::from_secs(599));
        let next_refresh = inner.status().next_refresh_at.unwrap();
        assert!(next_refresh >= epoch_millis() + 599_000);
        for (id, body) in [("aged", Some(fixture())), ("not-modified", None)] {
            let cached = validate_usage_reply(
                UsageReply {
                    body,
                    cache: CachePolicy {
                        delay: 120,
                        from_cache: true,
                    },
                },
                account(),
                200,
            )
            .unwrap();
            assert!(service
                .apply_usage(&mut inner, id, 190, cached, None)
                .is_none());
            assert_eq!(inner.snapshot.as_ref().unwrap().fetched_at, 100);
            assert_eq!(inner.error.as_ref().unwrap().code, "cached_response");
            assert_eq!(inner.failures, 0);
        }
        let query = crate::storage::HistoryQuery {
            account: account_key(&account()),
            metric: "premium_interactions".into(),
            unit: "provider_quota_credit".into(),
            semantics_version: "copilot-premium-v1".into(),
            from: 0,
            to: 1000,
            limit: 10,
            cursor: None,
        };
        assert_eq!(service.storage.query(query).unwrap().observations.len(), 1);
        assert_eq!(
            service
                .storage
                .latest(&account_key(&account()))
                .unwrap()
                .unwrap()["fetched_at"],
            json!(100)
        );
    }
    #[tokio::test]
    async fn cached_account_switch_never_exposes_previous_accounts_snapshot() {
        let service = GitHubService::new(
            Arc::new(Storage::in_memory().unwrap()),
            &crate::network::ProxySettings::default(),
        )
        .unwrap();
        let mut inner = service.inner.lock().await;
        let first_account = account();
        let first = validate_usage_reply(
            UsageReply {
                body: Some(fixture()),
                cache: CachePolicy::default(),
            },
            first_account.clone(),
            100,
        )
        .unwrap();
        service.apply_usage(&mut inner, "account-a", 90, first, None);
        let second_account = Account {
            id: first_account.id + 1,
            login: "another-account".into(),
        };
        for (index, body) in [
            None,
            Some({
                let mut body = fixture();
                body["login"] = json!(second_account.login);
                body
            }),
        ]
        .into_iter()
        .enumerate()
        {
            // Start each check with A's legitimate retained observation.
            inner.account = Some(first_account.clone());
            inner.snapshot = Some(project_copilot(fixture(), &first_account, 100).unwrap());
            let cached = validate_usage_reply(
                UsageReply {
                    body,
                    cache: CachePolicy {
                        delay: 120,
                        from_cache: true,
                    },
                },
                second_account.clone(),
                200,
            )
            .unwrap();
            assert!(service
                .apply_usage(&mut inner, &format!("account-b-{index}"), 190, cached, None)
                .is_none());
            assert_eq!(inner.account.as_ref().unwrap().id, second_account.id);
            assert!(inner.snapshot.is_none());
            assert!(service
                .storage
                .latest(&account_key(&second_account))
                .unwrap()
                .is_none());
            assert_eq!(
                service
                    .storage
                    .latest(&account_key(&first_account))
                    .unwrap()
                    .unwrap()["fetched_at"],
                json!(100)
            );
        }
        // The first sign-in can wait for fresh usage without fabricating an initial sample.
        inner.account = None;
        inner.snapshot = None;
        service.apply_usage(
            &mut inner,
            "initial-cache",
            290,
            ValidatedUsage {
                account: second_account,
                snapshot: None,
                cache_delay: 30,
            },
            None,
        );
        assert!(inner.snapshot.is_none());
        assert_eq!(inner.error.as_ref().unwrap().code, "cached_response");
    }

    #[tokio::test]
    async fn clearing_cache_cannot_shorten_provider_backoff() {
        let service = GitHubService::new(
            Arc::new(Storage::in_memory().unwrap()),
            &crate::network::ProxySettings::default(),
        )
        .unwrap();
        let deadline = deadline_after(7200);
        service.inner.lock().await.next_refresh = Some(deadline);
        let (_, status) = service.clear_cache_inner(None).await.unwrap();
        assert_eq!(service.inner.lock().await.next_refresh, Some(deadline));
        assert!(status.next_refresh_at.unwrap() >= epoch_millis() + 7_199_000);
    }

    #[tokio::test]
    async fn transient_failure_retries_quickly_and_success_resets_the_streak() {
        let (service, _) = fake_service();
        let token = test_token(account());
        let mut inner = service.inner.lock().await;
        inner.token = Some(token.clone());
        inner.account = Some(account());
        let timestamp = epoch_millis();
        inner.snapshot = test_usage(account(), timestamp).snapshot;
        for (index, delay) in [5, 5, 5, 15, 30, 60, 120, 300].into_iter().enumerate() {
            let error = service
                .finish_refresh(
                    &mut inner,
                    &format!("retry-{index}"),
                    timestamp,
                    &token,
                    Err(GitHubError::new("network", "Network unavailable")),
                )
                .unwrap_err();
            assert_eq!(error.retry_after, Some(delay));
            assert_eq!(inner.snapshot.as_ref().unwrap().fetched_at, timestamp);
            let remaining = inner
                .next_refresh
                .unwrap()
                .saturating_duration_since(Instant::now());
            assert!(remaining <= Duration::from_secs(delay));
            assert!(remaining > Duration::from_secs(delay - 1));
        }
        service
            .finish_refresh(
                &mut inner,
                "recovered",
                timestamp,
                &token,
                Ok(test_usage(account(), timestamp)),
            )
            .unwrap();
        assert_eq!(inner.failures, 0);
        assert!(inner.error.is_none());
        let error = service
            .finish_refresh(
                &mut inner,
                "new-interruption",
                timestamp,
                &token,
                Err(GitHubError::new("network", "Network unavailable")),
            )
            .unwrap_err();
        assert_eq!(error.retry_after, Some(5));
        drop(inner);
        assert_eq!(service.refresh().await.unwrap_err().code, "cooldown");
    }

    #[test]
    fn short_provider_hints_cannot_shorten_failure_backoff() {
        for (failures, expected) in [
            (1, 5),
            (2, 5),
            (3, 5),
            (4, 15),
            (5, 30),
            (6, 60),
            (7, 120),
            (8, 300),
            (100, 300),
            (u32::MAX, 300),
        ] {
            assert_eq!(refresh_retry_delay(failures, None), expected);
            assert_eq!(refresh_retry_delay(failures, Some(1)), expected);
            assert_eq!(refresh_retry_delay(failures, Some(7200)), 7200);
        }
    }
    #[test]
    fn unlimited_quota_has_no_calculated_percentage() {
        let mut raw = fixture();
        raw["quota_snapshots"]["premium_interactions"]["unlimited"] = json!(true);
        assert!(project_copilot(raw, &account(), 0)
            .unwrap()
            .used_percent
            .is_none());
    }
    #[test]
    fn token_response_rejects_scope_expansion_and_unknown_token_types() {
        assert!(
            parse_token(&json!({"access_token":"test-only","token_type":"bearer","scope":""}))
                .is_ok()
        );
        assert_eq!(
            parse_token(&json!({"access_token":"test-only","token_type":"bearer","scope":"repo"}))
                .err()
                .unwrap()
                .code,
            "unexpected_scope"
        );
        assert!(parse_token(&json!({"access_token":"test-only","token_type":"unknown"})).is_err());
        assert!(parse_token(
            &json!({"access_token":"test-only","token_type":"bearer","scope":"","expires_in":3600})
        )
        .is_ok());
    }
    #[test]
    fn poll_delay_rounds_up_and_terminal_oauth_errors_are_distinct() {
        let now = Instant::now();
        assert_eq!(seconds_until(now + Duration::from_millis(1200), now), 2);
        assert_eq!(oauth_error("access_denied").code, "authorization_denied");
        assert_eq!(oauth_error("expired_token").code, "device_expired");
        assert_eq!(oauth_error("device_flow_disabled").code, "configuration");
    }
    #[tokio::test]
    async fn cancellation_invalidates_inflight_epoch_and_clears_device() {
        let service = GitHubService::new(
            Arc::new(Storage::in_memory().unwrap()),
            &crate::network::ProxySettings::default(),
        )
        .unwrap();
        let (generation, mut cancelled) = service.reserve().await.unwrap();
        service.cancel().await;
        assert!(cancelled.changed().await.is_ok());
        let inner = service.inner.lock().await;
        assert_ne!(inner.generation, generation);
        assert!(!inner.busy);
        assert!(inner.device.is_none());
    }
    #[tokio::test]
    async fn early_polls_do_not_reach_network_and_expired_sessions_stop() {
        let service = GitHubService::new(
            Arc::new(Storage::in_memory().unwrap()),
            &crate::network::ProxySettings::default(),
        )
        .unwrap();
        {
            let mut inner = service.inner.lock().await;
            inner.device = Some(DeviceSession {
                code: "test-only".into(),
                expires: Instant::now() + Duration::from_secs(60),
                interval: 5,
                next_poll: Instant::now() + Duration::from_secs(5),
            });
        }
        let result = service.poll().await.unwrap();
        assert_eq!(result.state, "pending");
        assert_eq!(result.retry_after, Some(5));
        service.inner.lock().await.device.as_mut().unwrap().expires = Instant::now();
        assert_eq!(service.poll().await.err().unwrap().code, "device_expired");
        assert!(service.inner.lock().await.device.is_none());
    }
    #[tokio::test]
    async fn cooldown_and_revocation_block_network_requests() {
        let service = GitHubService::new(
            Arc::new(Storage::in_memory().unwrap()),
            &crate::network::ProxySettings::default(),
        )
        .unwrap();
        service.inner.lock().await.next_refresh = Some(Instant::now() + Duration::from_secs(30));
        assert_eq!(service.refresh().await.unwrap_err().code, "cooldown");
        {
            let mut inner = service.inner.lock().await;
            inner.next_refresh = None;
            inner.error = Some(GitHubError::new("reauth_required", "Reconnect"));
        }
        assert_eq!(service.refresh().await.unwrap_err().code, "reauth_required");
    }
    #[test]
    fn post_exchange_transient_errors_require_new_sign_in() {
        for code in ["network", "rate_limited", "http"] {
            let error = post_exchange_error(GitHubError::new(code, "Original diagnostic"));
            assert_eq!(error.code, "validation_failed");
            assert!(error.message.contains("Start sign-in again"));
        }
        for code in [
            "permission_denied",
            "reauth_required",
            "cancelled",
            "identity_mismatch",
        ] {
            let error = GitHubError::new(code, "Specific actionable error");
            assert_eq!(post_exchange_error(error.clone()), error);
        }
    }
    #[test]
    fn response_errors_distinguish_expiry_policy_and_rate_limit() {
        assert_eq!(classify_http(401, None, None).code, "reauth_required");
        assert_eq!(classify_http(403, None, None).code, "permission_denied");
        assert_eq!(classify_http(403, Some("0"), None).code, "rate_limited");
        assert_eq!(classify_http(403, None, Some(60)).retry_after, Some(60));
        assert_eq!(classify_http(429, None, None).code, "rate_limited");
        assert_eq!(classify_http(500, None, None).code, "http");
    }
    #[test]
    fn lifecycle_metadata_accepts_usable_tokens_without_persisting_refresh_secrets() {
        for extra in [
            json!({}),
            json!({"expires_in": null}),
            json!({"expires_in": 0}),
            json!({"expires_in": 3600, "refresh_token": "refresh-secret", "refresh_token_expires_in": 7200}),
        ] {
            let mut raw = json!({"access_token":"access-secret","token_type":"bearer","scope":""});
            raw.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let token = parse_token(&raw).unwrap();
            let saved = serde_json::to_string(&token).unwrap();
            assert!(!saved.contains("refresh-secret"));
            let restored = restore_token(&saved).unwrap();
            assert_eq!(restored.access_token, "access-secret");
            assert_eq!(restored.expires_at, token.expires_at);
        }
        assert_eq!(
            restore_token("legacy-token").unwrap().access_token,
            "legacy-token"
        );
        assert_eq!(
            token_expiration(&json!({"expires_in": 3600}), 1000).unwrap(),
            Some(3601000)
        );
        assert!(token_expiration(&json!({"expires_in": -1}), 0).is_err());
        assert!(token_expiration(&json!({"expires_in": "invalid"}), 0).is_err());
        assert!(token_expiration(&json!({"expires_in": u64::MAX}), 0).is_err());
    }
    #[tokio::test]
    async fn expired_saved_token_requires_reconnect_before_network_access() {
        let service = GitHubService::new(
            Arc::new(Storage::in_memory().unwrap()),
            &crate::network::ProxySettings::default(),
        )
        .unwrap();
        service.inner.lock().await.token = Some(StoredToken {
            access_token: "test-only".into(),
            expires_at: Some(1),
            account: None,
        });
        assert_eq!(service.refresh().await.unwrap_err().code, "reauth_required");
        assert_eq!(service.inner.lock().await.status().state, "reauth_required");
        assert!(!service.inner.lock().await.busy);
    }
}
