//! Native GitHub device flow. Secrets never implement Serialize or Debug.
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::State;
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
    pub entitlement: Option<f64>,
    pub credits_used: Option<f64>,
    pub quota_remaining: Option<f64>,
    pub percent_remaining: Option<f64>,
    pub overage_permitted: Option<bool>,
}
#[derive(Clone, Serialize, Debug)]
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
        })
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
}
impl Inner {
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
            state: state.into(),
            account: self.account.clone(),
            snapshot: self.snapshot.clone(),
            error: self.error.clone(),
        }
    }
}
pub struct GitHubService {
    client: Client,
    client_id: String,
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
    pub fn new() -> Result<Self, GitHubError> {
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
        let client = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(25))
            .user_agent("Luma/0.1.0")
            .build()
            .map_err(|_| GitHubError::new("network", "Cannot initialize HTTPS."))?;
        let (cancel, _) = watch::channel(0);
        Ok(Self {
            client,
            client_id: config.client_id,
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
            }),
        })
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
        tokio::select! {
            biased;
            _ = cancel.changed() => Err(GitHubError::cancelled()),
            result = async {
                let response = request.header("Accept", "application/json").send().await
                    .map_err(|_| GitHubError::new("network", "Cannot reach GitHub. Check the connection and retry."))?;
                if !response.status().is_success() { return Err(http_error(&response)); }
                let bytes = bounded_body(response).await?;
                serde_json::from_slice(&bytes).map_err(|_| GitHubError::schema())
            } => result,
        }
    }
    async fn validate(
        &self,
        token: &str,
        cancel: &mut watch::Receiver<u64>,
    ) -> Result<(Account, CopilotSnapshot), GitHubError> {
        let raw = self
            .request(
                self.client
                    .get("https://api.github.com/user")
                    .bearer_auth(token),
                cancel,
            )
            .await?;
        let account: Account = serde_json::from_value(raw).map_err(|_| GitHubError::schema())?;
        if account.id == 0 || account.login.is_empty() {
            return Err(GitHubError::schema());
        }
        let raw = self
            .request(
                self.client
                    .get("https://api.github.com/copilot_internal/user")
                    .bearer_auth(token),
                cancel,
            )
            .await?;
        let snapshot = project_copilot(raw, &account, epoch_millis())?;
        Ok((account, snapshot))
    }
    async fn status(&self) -> AuthStatus {
        // Serialize concurrent startup status calls until credential validation has completed.
        let _restore = self.restore.lock().await;
        let should_restore = {
            let mut inner = self.inner.lock().await;
            if inner.loaded {
                false
            } else {
                inner.loaded = true;
                match credential().and_then(|entry| match entry.get_password() {
                    Ok(token) if !token.is_empty() => restore_token(&token).map(Some),
                    Ok(_) | Err(keyring::Error::NoEntry) => Ok(None),
                    Err(_) => Err(GitHubError::storage()),
                }) {
                    Ok(token) => inner.token = token,
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
                    self.client
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
            Ok(response) => {
                let interval = response.interval.unwrap_or(5);
                let now = Instant::now();
                inner.device = Some(DeviceSession {
                    code: response.device_code,
                    expires: now + Duration::from_secs(response.expires_in),
                    interval,
                    next_poll: now + Duration::from_secs(interval),
                });
                inner.error = None;
                Ok(DeviceAuthorization {
                    user_code: response.user_code,
                    verification_uri: response.verification_uri,
                    expires_in: response.expires_in,
                    interval,
                })
            }
            Err(error) => {
                inner.error = Some(error.clone());
                Err(error)
            }
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
                self.client
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
                            .max(device.interval)
                            .min(3600);
                        device.next_poll = Instant::now() + Duration::from_secs(delay);
                        error.retry_after = Some(delay);
                    }
                    inner.error = Some(error.clone());
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
            inner.error = Some(error.clone());
            return Err(error);
        }
        let result = async {
            let token = parse_token(&raw)?;
            let (account, snapshot) = self
                .validate(&token.access_token, &mut cancel)
                .await
                .map_err(post_exchange_error)?;
            Ok::<_, GitHubError>((token, account, snapshot))
        }
        .await;
        let mut inner = self.inner.lock().await;
        if generation != inner.generation {
            return Err(GitHubError::cancelled());
        }
        inner.busy = false;
        inner.device = None;
        let (token, account, snapshot) = match result {
            Ok(values) => values,
            Err(error) => {
                inner.error = Some(error.clone());
                return Err(error);
            }
        };
        // Serialize credential mutation with disconnect so late responses cannot restore a deleted token.
        if let Err(error) = credential().and_then(|entry| {
            entry
                .set_password(&serde_json::to_string(&token).map_err(|_| GitHubError::storage())?)
                .map_err(|_| GitHubError::storage())
        }) {
            inner.error = Some(error.clone());
            return Err(error);
        }
        inner.token = Some(token);
        inner.loaded = true;
        inner.account = Some(account);
        inner.snapshot = Some(snapshot);
        inner.error = None;
        inner.next_refresh = Some(Instant::now() + Duration::from_secs(30));
        inner.failures = 0;
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
        let result = self.validate(&token.access_token, &mut cancel).await;
        let mut inner = self.inner.lock().await;
        if generation != inner.generation {
            return Err(GitHubError::cancelled());
        }
        inner.busy = false;
        match result {
            Ok((account, snapshot)) => {
                if inner
                    .account
                    .as_ref()
                    .is_some_and(|old| old.id != account.id)
                {
                    let error = GitHubError::new(
                        "identity_mismatch",
                        "GitHub identity changed. Reconnect before collecting usage.",
                    );
                    inner.error = Some(error.clone());
                    return Err(error);
                }
                inner.account = Some(account);
                inner.snapshot = Some(snapshot.clone());
                inner.error = None;
                inner.next_refresh = Some(Instant::now() + Duration::from_secs(30));
                inner.failures = 0;
                Ok(snapshot)
            }
            Err(mut error) => {
                inner.failures = inner.failures.saturating_add(1);
                let delay = error
                    .retry_after
                    .unwrap_or(30_u64.saturating_mul(1_u64 << inner.failures.min(6)))
                    .max(30);
                inner.next_refresh = Instant::now().checked_add(Duration::from_secs(delay));
                error.retry_after = Some(delay);
                inner.error = Some(error.clone());
                Err(error)
            }
        }
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
        inner.failures = 0;
        let deletion = credential().and_then(|entry| match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(GitHubError::storage()),
        });
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
    classify_http(
        response.status().as_u16(),
        response
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok()),
        response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok()),
    )
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
    if [
        premium.entitlement,
        premium.credits_used,
        premium.quota_remaining,
        premium.percent_remaining,
    ]
    .into_iter()
    .flatten()
    .any(|v| !v.is_finite())
        || premium.entitlement.is_some_and(|v| v < 0.0)
        || premium.credits_used.is_some_and(|v| v < 0.0)
    {
        return Err(GitHubError::schema());
    }
    let used_percent = match (premium.credits_used, premium.entitlement) {
        (Some(used), Some(limit)) if limit > 0.0 => {
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
pub async fn github_poll(service: State<'_, GitHubService>) -> Result<PollResult, GitHubError> {
    service.poll().await
}
#[tauri::command]
pub async fn github_cancel(service: State<'_, GitHubService>) -> Result<(), GitHubError> {
    service.cancel().await;
    Ok(())
}
#[tauri::command]
pub async fn github_disconnect(
    service: State<'_, GitHubService>,
) -> Result<AuthStatus, GitHubError> {
    service.disconnect().await
}
#[tauri::command]
pub async fn github_refresh(
    service: State<'_, GitHubService>,
) -> Result<CopilotSnapshot, GitHubError> {
    service.refresh().await
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
    #[test]
    fn preserves_provider_values_and_calculates_independent_percentage() {
        let result = project_copilot(fixture(), &account(), 123).unwrap();
        assert!((result.used_percent.unwrap() - 36.1171).abs() < 0.000001);
        assert_eq!(result.premium.quota_remaining, Some(1277551.1));
        assert_eq!(result.premium.percent_remaining, Some(63.8));
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
        let service = GitHubService::new().unwrap();
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
        let service = GitHubService::new().unwrap();
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
        let service = GitHubService::new().unwrap();
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
        let service = GitHubService::new().unwrap();
        service.inner.lock().await.token = Some(StoredToken {
            access_token: "test-only".into(),
            expires_at: Some(1),
        });
        assert_eq!(service.refresh().await.unwrap_err().code, "reauth_required");
        assert_eq!(service.inner.lock().await.status().state, "reauth_required");
        assert!(!service.inner.lock().await.busy);
    }
}
