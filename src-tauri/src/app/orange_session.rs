use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::sync::Mutex;

use crate::app::atomic_file;
use crate::app::orange_api::{LoginOutcome, OrangeApiClient, OrangeError, OrangeProxy, TokenPair};

pub use crate::app::orange_api::{RawApiKey, RawGroup};

const REFRESH_EARLY: Duration = Duration::from_secs(30);
const CREDENTIAL_SERVICE: &str = "io.github.wangnov.codexappmanager";
const CREDENTIAL_USER: &str = "orangeapi-refresh-token";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrangeConnection {
    SignedOut,
    Connected,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrangeKeyStatus {
    Active,
    Inactive,
    QuotaExhausted,
    Expired,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrangeSessionView {
    pub authenticated: bool,
    pub email: Option<String>,
    pub remembered: bool,
    pub connection: OrangeConnection,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrangeKeyView {
    pub id: u64,
    pub name: String,
    pub group_name: String,
    pub masked_key: String,
    pub status: OrangeKeyStatus,
    pub quota: f64,
    pub quota_used: f64,
    pub expires_at: Option<String>,
    pub actionable: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OrangeKeyList {
    pub items: Vec<OrangeKeyView>,
    pub stale: bool,
    pub fetched_at_unix: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionMetadata {
    last_email: Option<String>,
    remember_login: bool,
    #[serde(default)]
    site_name: Option<String>,
}

trait RefreshTokenStore: Send + Sync {
    fn load(&self) -> Result<Option<String>, OrangeError>;
    fn save(&self, token: &str) -> Result<(), OrangeError>;
    fn clear(&self) -> Result<(), OrangeError>;
}

struct OsRefreshTokenStore;

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl OsRefreshTokenStore {
    fn entry() -> Result<keyring::Entry, OrangeError> {
        keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
            .map_err(|_| OrangeError::CredentialStore)
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl RefreshTokenStore for OsRefreshTokenStore {
    fn load(&self) -> Result<Option<String>, OrangeError> {
        match Self::entry()?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(OrangeError::CredentialStore),
        }
    }

    fn save(&self, token: &str) -> Result<(), OrangeError> {
        Self::entry()?
            .set_password(token)
            .map_err(|_| OrangeError::CredentialStore)
    }

    fn clear(&self) -> Result<(), OrangeError> {
        match Self::entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(OrangeError::CredentialStore),
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
impl RefreshTokenStore for OsRefreshTokenStore {
    fn load(&self) -> Result<Option<String>, OrangeError> {
        Ok(None)
    }

    fn save(&self, _token: &str) -> Result<(), OrangeError> {
        Err(OrangeError::UnsupportedPlatform)
    }

    fn clear(&self) -> Result<(), OrangeError> {
        Ok(())
    }
}

#[derive(Default)]
struct SessionState {
    proxy: Option<OrangeProxy>,
    client: Option<OrangeApiClient>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_at: Option<Instant>,
    email: Option<String>,
    site_name: Option<String>,
    warning: Option<String>,
    restore_attempted: bool,
    keys: Vec<RawApiKey>,
    projected: OrangeKeyList,
}

pub struct OrangeSessionService {
    state: Mutex<SessionState>,
    metadata: Mutex<SessionMetadata>,
    metadata_path: Option<PathBuf>,
    token_store: Arc<dyn RefreshTokenStore>,
}

impl OrangeSessionService {
    pub fn new() -> Self {
        let metadata_path = crate::app::paths::orange_session_path();
        let metadata = metadata_path
            .as_deref()
            .and_then(load_metadata)
            .unwrap_or_default();
        Self {
            state: Mutex::new(SessionState {
                email: metadata.last_email.clone(),
                site_name: metadata.site_name.clone(),
                ..SessionState::default()
            }),
            metadata: Mutex::new(metadata),
            metadata_path,
            token_store: Arc::new(OsRefreshTokenStore),
        }
    }

    #[cfg(test)]
    fn for_test(
        metadata_path: PathBuf,
        token_store: Arc<dyn RefreshTokenStore>,
        metadata: SessionMetadata,
    ) -> Self {
        Self {
            state: Mutex::new(SessionState {
                email: metadata.last_email.clone(),
                site_name: metadata.site_name.clone(),
                ..SessionState::default()
            }),
            metadata: Mutex::new(metadata),
            metadata_path: Some(metadata_path),
            token_store,
        }
    }

    pub async fn session_view(&self, proxy: OrangeProxy) -> Result<OrangeSessionView, OrangeError> {
        let mut state = self.state.lock().await;
        self.ensure_client(&mut state, proxy)?;
        if state.access_token.is_none() && !state.restore_attempted {
            state.restore_attempted = true;
            let remembered = self.metadata.lock().await.remember_login;
            if remembered {
                match self.token_store.load() {
                    Ok(Some(refresh_token)) => {
                        state.refresh_token = Some(refresh_token);
                        if let Err(error) = self.refresh_locked(&mut state).await {
                            if refresh_error_ends_session(&error) {
                                self.clear_auth_locked(&mut state);
                                let _ = self.token_store.clear();
                                let warning = OrangeError::SignedOut.code().to_string();
                                state.warning = Some(
                                    self.set_remembered(false)
                                        .await
                                        .err()
                                        .map_or(warning, |error| error.code().to_string()),
                                );
                            } else {
                                state.warning = Some(error.code().to_string());
                                state.restore_attempted = false;
                            }
                        }
                    }
                    Ok(None) => {
                        if let Err(error) = self.set_remembered(false).await {
                            state.warning = Some(error.code().into());
                        }
                    }
                    Err(error) => state.warning = Some(error.code().to_string()),
                }
            }
        }
        Ok(self.view_locked(&state).await)
    }

    pub async fn login(
        &self,
        proxy: OrangeProxy,
        email: String,
        password: String,
        remember: bool,
    ) -> Result<OrangeSessionView, OrangeError> {
        let mut state = self.state.lock().await;
        self.ensure_client(&mut state, proxy)?;
        let client = state.client.clone().ok_or(OrangeError::Network)?;
        let public_settings = client.public_settings().await?;
        if public_settings.turnstile_enabled {
            return Err(OrangeError::TurnstileUnsupported);
        }
        let LoginOutcome::Authenticated(tokens) = client.login(&email, &password).await? else {
            return Err(OrangeError::TwoFactorUnsupported);
        };

        let refresh_token = tokens.refresh_token.clone();
        let expires_at = tokens.expires_in.map(expiry_after).transpose()?;
        state.access_token = Some(tokens.access_token);
        state.refresh_token = refresh_token.clone();
        state.expires_at = expires_at;
        state.email = Some(tokens.user.email.clone());
        state.site_name = non_empty(public_settings.site_name);
        state.warning = None;
        state.restore_attempted = true;
        state.keys.clear();
        state.projected = OrangeKeyList::default();

        let metadata_persisted = {
            let mut metadata = self.metadata.lock().await;
            metadata.last_email = Some(tokens.user.email);
            metadata.remember_login = remember && refresh_token.is_some();
            metadata.site_name = state.site_name.clone();
            match persist_metadata(self.metadata_path.as_deref(), &metadata) {
                Ok(()) => true,
                Err(error) => {
                    metadata.remember_login = false;
                    state.warning = Some(error.code().into());
                    false
                }
            }
        };

        if remember && metadata_persisted {
            match refresh_token {
                Some(token) => {
                    if let Err(error) = self.token_store.save(&token) {
                        state.warning = Some(
                            self.set_remembered(false)
                                .await
                                .err()
                                .unwrap_or(error)
                                .code()
                                .to_string(),
                        );
                    }
                }
                None => {
                    state.warning = Some(self.set_remembered(false).await.err().map_or_else(
                        || "orange_refresh_unavailable".into(),
                        |error| error.code().into(),
                    ));
                }
            }
        } else if self.token_store.clear().is_err() && state.warning.is_none() {
            state.warning = Some(OrangeError::CredentialStore.code().into());
        }
        Ok(self.view_locked(&state).await)
    }

    pub async fn refresh_keys(
        &self,
        local_key: Option<&str>,
    ) -> Result<OrangeKeyList, OrangeError> {
        let mut state = self.state.lock().await;
        if state.access_token.is_none() {
            return Err(OrangeError::SignedOut);
        }
        let previous = state.projected.clone();
        if self.token_needs_refresh(&state) {
            if let Err(error) = self.refresh_locked(&mut state).await {
                if refresh_error_ends_session(&error) {
                    self.expire_session_locked(&mut state).await;
                    return Err(OrangeError::SignedOut);
                }
                state.warning = Some(error.code().into());
                if previous.items.is_empty() {
                    return Err(error);
                }
                state.projected = OrangeKeyList {
                    stale: true,
                    ..previous
                };
                return Ok(state.projected.clone());
            }
        }

        match self.fetch_all_keys_locked(&mut state).await {
            Ok(mut keys) => {
                keys.retain(|key| {
                    key.group
                        .as_ref()
                        .is_some_and(|group| group.platform.eq_ignore_ascii_case("openai"))
                });
                sort_keys_newest_first(&mut keys);
                let now = unix_now();
                let items = keys
                    .iter()
                    .map(|key| project_key(key, local_key, now))
                    .collect();
                state.keys = keys;
                clear_transient_request_warning(&mut state.warning);
                state.projected = OrangeKeyList {
                    items,
                    stale: false,
                    fetched_at_unix: now.max(0) as u64,
                };
                Ok(state.projected.clone())
            }
            Err(error)
                if !previous.items.is_empty()
                    && !matches!(error, OrangeError::Unauthorized | OrangeError::SignedOut) =>
            {
                state.warning = Some(error.code().into());
                state.projected = OrangeKeyList {
                    stale: true,
                    ..previous
                };
                Ok(state.projected.clone())
            }
            Err(error) => Err(error),
        }
    }

    pub async fn logout(&self) -> Result<OrangeSessionView, OrangeError> {
        let mut state = self.state.lock().await;
        if let (Some(client), Some(refresh_token)) =
            (state.client.clone(), state.refresh_token.clone())
        {
            let _ = client.logout(&refresh_token).await;
        }
        self.clear_auth_locked(&mut state);
        state.restore_attempted = true;
        let credential_warning = self.token_store.clear().err();
        if let Err(error) = self.set_remembered(false).await {
            state.warning = Some(error.code().into());
        } else {
            state.warning = credential_warning.map(|error| error.code().into());
        }
        Ok(self.view_locked(&state).await)
    }

    pub async fn full_key(&self, id: u64) -> Result<String, OrangeError> {
        let state = self.state.lock().await;
        if state.projected.stale {
            return Err(OrangeError::KeyUnavailable);
        }
        state
            .keys
            .iter()
            .find(|key| key.id == id && key.actionable_at(unix_now()))
            .map(|key| key.key.clone())
            .ok_or(OrangeError::KeyUnavailable)
    }

    pub async fn provider_name(&self) -> String {
        self.state
            .lock()
            .await
            .site_name
            .clone()
            .unwrap_or_else(|| "OrangeAPI".into())
    }

    pub async fn mark_enabled(&self, id: u64) {
        let mut state = self.state.lock().await;
        for key in &mut state.projected.items {
            key.enabled = key.id == id;
        }
    }

    fn ensure_client(
        &self,
        state: &mut SessionState,
        proxy: OrangeProxy,
    ) -> Result<(), OrangeError> {
        if state.proxy.as_ref() != Some(&proxy) {
            state.client = Some(OrangeApiClient::new(proxy.clone())?);
            state.proxy = Some(proxy);
        }
        Ok(())
    }

    fn token_needs_refresh(&self, state: &SessionState) -> bool {
        state
            .expires_at
            .is_some_and(|expiry| expiry <= Instant::now() + REFRESH_EARLY)
    }

    async fn refresh_locked(&self, state: &mut SessionState) -> Result<(), OrangeError> {
        let refresh_token = state.refresh_token.clone().ok_or(OrangeError::SignedOut)?;
        let client = state.client.clone().ok_or(OrangeError::Network)?;
        let tokens = client.refresh(&refresh_token).await?;
        self.apply_refreshed_tokens(state, tokens).await
    }

    async fn apply_refreshed_tokens(
        &self,
        state: &mut SessionState,
        tokens: TokenPair,
    ) -> Result<(), OrangeError> {
        let expires_at = expiry_after(tokens.expires_in)?;
        state.access_token = Some(tokens.access_token);
        state.refresh_token = Some(tokens.refresh_token.clone());
        state.expires_at = Some(expires_at);
        clear_transient_request_warning(&mut state.warning);
        let remembered = self.metadata.lock().await.remember_login;
        if remembered {
            if let Err(error) = self.token_store.save(&tokens.refresh_token) {
                state.warning = Some(
                    self.set_remembered(false)
                        .await
                        .err()
                        .unwrap_or(error)
                        .code()
                        .into(),
                );
            }
        }
        Ok(())
    }

    async fn fetch_all_keys_locked(
        &self,
        state: &mut SessionState,
    ) -> Result<Vec<RawApiKey>, OrangeError> {
        let mut page = 1u32;
        let mut pages = 1u32;
        let mut retried = false;
        let mut all = Vec::new();
        while page <= pages {
            let client = state.client.clone().ok_or(OrangeError::Network)?;
            let access_token = state
                .access_token
                .as_deref()
                .ok_or(OrangeError::SignedOut)?;
            let response = client.key_page(access_token, page).await;
            let result = match response {
                Err(OrangeError::Unauthorized) if !retried => {
                    retried = true;
                    if let Err(error) = self.refresh_locked(state).await {
                        if refresh_error_ends_session(&error) {
                            self.expire_session_locked(state).await;
                            return Err(OrangeError::SignedOut);
                        }
                        return Err(error);
                    }
                    let access_token = state
                        .access_token
                        .as_deref()
                        .ok_or(OrangeError::SignedOut)?;
                    client.key_page(access_token, page).await
                }
                other => other,
            };
            let page_data = match result {
                Err(OrangeError::Unauthorized) => {
                    self.expire_session_locked(state).await;
                    return Err(OrangeError::SignedOut);
                }
                other => other?,
            };
            if page_data.pages > 1_000
                || (page_data.pages > 0 && page_data.page != page)
                || page_data.page_size == 0
            {
                return Err(OrangeError::InvalidResponse);
            }
            pages = page_data.pages;
            all.extend(page_data.items);
            if pages == 0 {
                break;
            }
            page = page.checked_add(1).ok_or(OrangeError::InvalidResponse)?;
        }
        Ok(all)
    }

    fn clear_auth_locked(&self, state: &mut SessionState) {
        state.access_token = None;
        state.refresh_token = None;
        state.expires_at = None;
        state.site_name = None;
        state.keys.clear();
        state.projected = OrangeKeyList::default();
    }

    async fn expire_session_locked(&self, state: &mut SessionState) {
        self.clear_auth_locked(state);
        state.warning = Some(OrangeError::SignedOut.code().into());
        let _ = self.token_store.clear();
        if let Err(error) = self.set_remembered(false).await {
            state.warning = Some(error.code().into());
        }
    }

    async fn set_remembered(&self, remembered: bool) -> Result<(), OrangeError> {
        let mut metadata = self.metadata.lock().await;
        metadata.remember_login = remembered;
        persist_metadata(self.metadata_path.as_deref(), &metadata)
    }

    async fn view_locked(&self, state: &SessionState) -> OrangeSessionView {
        let metadata = self.metadata.lock().await;
        let authenticated = state.access_token.is_some();
        OrangeSessionView {
            authenticated,
            email: state.email.clone().or_else(|| metadata.last_email.clone()),
            remembered: metadata.remember_login,
            connection: if !authenticated {
                OrangeConnection::SignedOut
            } else if state.warning.is_some() {
                OrangeConnection::Interrupted
            } else {
                OrangeConnection::Connected
            },
            warning: state.warning.clone(),
        }
    }
}

fn expiry_after(seconds: u64) -> Result<Instant, OrangeError> {
    Instant::now()
        .checked_add(Duration::from_secs(seconds))
        .ok_or(OrangeError::InvalidResponse)
}

fn refresh_error_ends_session(error: &OrangeError) -> bool {
    matches!(error, OrangeError::Unauthorized | OrangeError::SignedOut)
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn clear_transient_request_warning(warning: &mut Option<String>) {
    if warning.as_deref().is_some_and(|code| {
        matches!(
            code,
            "orange_forbidden"
                | "orange_rate_limited"
                | "orange_timeout"
                | "orange_network"
                | "orange_invalid_response"
                | "orange_api_rejected"
        )
    }) {
        *warning = None;
    }
}

impl Default for OrangeSessionService {
    fn default() -> Self {
        Self::new()
    }
}

impl RawApiKey {
    pub fn actionable_at(&self, now_unix: i64) -> bool {
        let group_active = self
            .group
            .as_ref()
            .is_some_and(|group| group.status == "active");
        let not_expired = self.expires_at.as_deref().is_none_or(|value| {
            OffsetDateTime::parse(value, &Rfc3339)
                .map(|expiry| expiry.unix_timestamp() > now_unix)
                .unwrap_or(false)
        });
        self.status == "active"
            && group_active
            && not_expired
            && (self.quota <= 0.0 || self.quota_used < self.quota)
    }

    fn effective_status(&self, now_unix: i64) -> OrangeKeyStatus {
        let expiration = self.expires_at.as_deref().map(|value| {
            OffsetDateTime::parse(value, &Rfc3339).map(|expiry| expiry.unix_timestamp() <= now_unix)
        });
        if matches!(expiration, Some(Ok(true))) || self.status == "expired" {
            OrangeKeyStatus::Expired
        } else if matches!(expiration, Some(Err(_))) {
            OrangeKeyStatus::Inactive
        } else if !self
            .group
            .as_ref()
            .is_some_and(|group| group.status == "active")
        {
            OrangeKeyStatus::Inactive
        } else if (self.quota > 0.0 && self.quota_used >= self.quota)
            || self.status == "quota_exhausted"
        {
            OrangeKeyStatus::QuotaExhausted
        } else if self.status == "active" {
            OrangeKeyStatus::Active
        } else {
            OrangeKeyStatus::Inactive
        }
    }
}

fn sort_keys_newest_first(keys: &mut [RawApiKey]) {
    keys.sort_by(|left, right| {
        let left_time = OffsetDateTime::parse(&left.created_at, &Rfc3339)
            .map(|value| value.unix_timestamp_nanos());
        let right_time = OffsetDateTime::parse(&right.created_at, &Rfc3339)
            .map(|value| value.unix_timestamp_nanos());
        match (left_time, right_time) {
            (Ok(left_time), Ok(right_time)) => right_time
                .cmp(&left_time)
                .then_with(|| right.id.cmp(&left.id)),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => right.id.cmp(&left.id),
        }
    });
}

pub fn mask_api_key(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 8 {
        return "*****".into();
    }
    let prefix: String = chars.iter().take(4).collect();
    let suffix: String = chars.iter().skip(chars.len() - 3).collect();
    format!("{prefix}*****{suffix}")
}

fn project_key(key: &RawApiKey, local_key: Option<&str>, now: i64) -> OrangeKeyView {
    let actionable = key.actionable_at(now);
    OrangeKeyView {
        id: key.id,
        name: key.name.clone(),
        group_name: key
            .group
            .as_ref()
            .map(|group| group.name.clone())
            .unwrap_or_else(|| "-".into()),
        masked_key: mask_api_key(&key.key),
        status: key.effective_status(now),
        quota: key.quota,
        quota_used: key.quota_used,
        expires_at: key.expires_at.clone(),
        actionable,
        enabled: local_key == Some(key.key.as_str()),
    }
}

fn load_metadata(path: &Path) -> Option<SessionMetadata> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn persist_metadata(path: Option<&Path>, metadata: &SessionMetadata) -> Result<(), OrangeError> {
    let Some(path) = path else {
        return Err(OrangeError::Persistence);
    };
    let bytes = serde_json::to_vec(metadata).map_err(|_| OrangeError::Persistence)?;
    atomic_file::write_atomic(path, &bytes).map_err(|_| OrangeError::Persistence)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use httpmock::prelude::*;

    use super::*;

    fn key(status: &str, group_status: &str, quota: f64, used: f64) -> RawApiKey {
        RawApiKey {
            id: 1,
            key: "sk-1234567890".into(),
            name: "Primary".into(),
            status: status.into(),
            quota,
            quota_used: used,
            expires_at: None,
            created_at: "2026-07-30T00:00:00Z".into(),
            group: Some(RawGroup {
                name: "OpenAI".into(),
                platform: "openai".into(),
                status: group_status.into(),
            }),
        }
    }

    #[derive(Default)]
    struct MemoryTokenStore(StdMutex<Option<String>>);

    impl RefreshTokenStore for MemoryTokenStore {
        fn load(&self) -> Result<Option<String>, OrangeError> {
            Ok(self.0.lock().unwrap().clone())
        }

        fn save(&self, token: &str) -> Result<(), OrangeError> {
            *self.0.lock().unwrap() = Some(token.into());
            Ok(())
        }

        fn clear(&self) -> Result<(), OrangeError> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }

    struct FailingSaveTokenStore;

    impl RefreshTokenStore for FailingSaveTokenStore {
        fn load(&self) -> Result<Option<String>, OrangeError> {
            Ok(None)
        }

        fn save(&self, _token: &str) -> Result<(), OrangeError> {
            Err(OrangeError::CredentialStore)
        }

        fn clear(&self) -> Result<(), OrangeError> {
            Ok(())
        }
    }

    #[test]
    fn masks_without_leaking_short_or_long_keys() {
        assert_eq!(mask_api_key("sk-1234567890"), "sk-1*****890");
        assert_eq!(mask_api_key("short"), "*****");
    }

    #[test]
    fn actionability_requires_active_group_unexpired_key_and_quota() {
        assert!(key("active", "active", 0.0, 99.0).actionable_at(1_000));
        assert!(!key("inactive", "active", 0.0, 0.0).actionable_at(1_000));
        assert!(!key("active", "inactive", 0.0, 0.0).actionable_at(1_000));
        assert!(!key("active", "active", 10.0, 10.0).actionable_at(1_000));
        assert_eq!(
            key("active", "inactive", 0.0, 0.0).effective_status(1_000),
            OrangeKeyStatus::Inactive
        );
    }

    #[test]
    fn invalid_expiration_never_projects_as_active() {
        let mut invalid = key("active", "active", 0.0, 0.0);
        invalid.expires_at = Some("not-a-date".into());

        assert_eq!(invalid.effective_status(1_000), OrangeKeyStatus::Inactive);
        assert!(!invalid.actionable_at(1_000));
    }

    #[test]
    fn local_key_match_is_enabled_even_when_the_remote_key_is_inactive() {
        let inactive = key("inactive", "active", 0.0, 0.0);

        let projected = project_key(&inactive, Some("sk-1234567890"), 1_000);

        assert!(projected.enabled);
        assert!(!projected.actionable);
    }

    #[test]
    fn sorts_keys_by_timestamp_instead_of_rfc3339_text() {
        let mut actually_older = key("active", "active", 0.0, 0.0);
        actually_older.id = 1;
        actually_older.created_at = "2026-01-01T00:30:00+01:00".into();
        let mut actually_newer = key("active", "active", 0.0, 0.0);
        actually_newer.id = 2;
        actually_newer.created_at = "2026-01-01T00:00:00Z".into();
        let mut keys = vec![actually_older, actually_newer];

        sort_keys_newest_first(&mut keys);

        assert_eq!(
            keys.iter().map(|key| key.id).collect::<Vec<_>>(),
            vec![2, 1]
        );
    }

    #[test]
    fn rejects_access_token_lifetime_that_overflows_instant() {
        assert_eq!(
            expiry_after(u64::MAX).unwrap_err(),
            OrangeError::InvalidResponse
        );
    }

    #[tokio::test]
    async fn successful_refresh_clears_a_transient_connection_warning() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("orange-refresh-warning-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let service = OrangeSessionService::for_test(
            root.join("session.json"),
            Arc::new(MemoryTokenStore::default()),
            SessionMetadata::default(),
        );
        let mut state = SessionState {
            warning: Some("orange_network".into()),
            ..SessionState::default()
        };

        service
            .apply_refreshed_tokens(
                &mut state,
                TokenPair {
                    access_token: "access".into(),
                    refresh_token: "refresh".into(),
                    expires_in: 3600,
                    token_type: Some("Bearer".into()),
                },
            )
            .await
            .unwrap();

        assert!(state.warning.is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn provider_name_falls_back_when_public_site_name_is_empty() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("orange-provider-name-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let service = OrangeSessionService::for_test(
            root.join("session.json"),
            Arc::new(MemoryTokenStore::default()),
            SessionMetadata::default(),
        );

        assert_eq!(service.provider_name().await, "OrangeAPI");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn restored_session_metadata_keeps_the_public_provider_name() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("orange-restored-name-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let service = OrangeSessionService::for_test(
            root.join("session.json"),
            Arc::new(MemoryTokenStore::default()),
            SessionMetadata {
                last_email: Some("a@b.test".into()),
                remember_login: true,
                site_name: Some("Cylon API".into()),
            },
        );

        assert_eq!(service.provider_name().await, "Cylon API");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn refresh_rejection_clears_the_local_session() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/api/v1/keys");
                then.status(401);
            })
            .await;
        server
            .mock_async(|when, then| {
                when.method(POST).path("/api/v1/auth/refresh");
                then.status(401);
            })
            .await;

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("orange-refresh-rejected-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let token_store = Arc::new(MemoryTokenStore(StdMutex::new(Some("refresh".into()))));
        let service = OrangeSessionService::for_test(
            root.join("session.json"),
            token_store.clone(),
            SessionMetadata {
                last_email: Some("a@b.test".into()),
                remember_login: true,
                site_name: None,
            },
        );
        {
            let mut state = service.state.lock().await;
            state.proxy = Some(OrangeProxy::Direct);
            state.client = Some(OrangeApiClient::for_test(server.base_url()));
            state.access_token = Some("expired-access".into());
            state.refresh_token = Some("refresh".into());
            state.email = Some("a@b.test".into());
        }

        let error = service.refresh_keys(None).await.unwrap_err();

        assert_eq!(error, OrangeError::SignedOut);
        let state = service.state.lock().await;
        assert!(state.access_token.is_none());
        assert!(state.refresh_token.is_none());
        assert_eq!(state.warning.as_deref(), Some("orange_signed_out"));
        drop(state);
        assert!(token_store.load().unwrap().is_none());
        assert!(!service.metadata.lock().await.remember_login);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn transient_refresh_failure_preserves_remembered_session_and_cached_keys() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(POST).path("/api/v1/auth/refresh");
                then.status(503);
            })
            .await;

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("orange-refresh-transient-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let token_store = Arc::new(MemoryTokenStore(StdMutex::new(Some("refresh".into()))));
        let service = OrangeSessionService::for_test(
            root.join("session.json"),
            token_store.clone(),
            SessionMetadata {
                last_email: Some("a@b.test".into()),
                remember_login: true,
                site_name: None,
            },
        );
        {
            let cached = project_key(&key("active", "active", 0.0, 0.0), None, unix_now());
            let mut state = service.state.lock().await;
            state.proxy = Some(OrangeProxy::Direct);
            state.client = Some(OrangeApiClient::for_test(server.base_url()));
            state.access_token = Some("expired-access".into());
            state.refresh_token = Some("refresh".into());
            state.expires_at = Some(Instant::now());
            state.email = Some("a@b.test".into());
            state.keys = vec![key("active", "active", 0.0, 0.0)];
            state.projected = OrangeKeyList {
                items: vec![cached],
                stale: false,
                fetched_at_unix: 1,
            };
        }

        let keys = service.refresh_keys(None).await.unwrap();

        assert!(keys.stale);
        assert_eq!(keys.items.len(), 1);
        assert_eq!(
            service.full_key(1).await.unwrap_err(),
            OrangeError::KeyUnavailable
        );
        let state = service.state.lock().await;
        assert_eq!(state.access_token.as_deref(), Some("expired-access"));
        assert_eq!(state.refresh_token.as_deref(), Some("refresh"));
        assert_eq!(state.warning.as_deref(), Some("orange_api_rejected"));
        drop(state);
        assert_eq!(token_store.load().unwrap().as_deref(), Some("refresh"));
        assert!(service.metadata.lock().await.remember_login);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn successful_key_fetch_preserves_a_credential_store_warning() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/api/v1/keys")
                    .header("authorization", "Bearer new-access");
                then.status(200).json_body_obj(&serde_json::json!({
                    "code": 0,
                    "data": {"items": [], "total": 0, "page": 1, "page_size": 50, "pages": 0}
                }));
            })
            .await;

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!(
                "orange-credential-warning-{}",
                uuid::Uuid::new_v4()
            ));
        fs::create_dir_all(&root).unwrap();
        let service = OrangeSessionService::for_test(
            root.join("session.json"),
            Arc::new(FailingSaveTokenStore),
            SessionMetadata {
                last_email: Some("a@b.test".into()),
                remember_login: true,
                site_name: None,
            },
        );
        {
            let mut state = service.state.lock().await;
            state.proxy = Some(OrangeProxy::Direct);
            state.client = Some(OrangeApiClient::for_test(server.base_url()));
            service
                .apply_refreshed_tokens(
                    &mut state,
                    TokenPair {
                        access_token: "new-access".into(),
                        refresh_token: "new-refresh".into(),
                        expires_in: 3600,
                        token_type: Some("Bearer".into()),
                    },
                )
                .await
                .unwrap();
            assert_eq!(
                state.warning.as_deref(),
                Some(OrangeError::CredentialStore.code())
            );
        }

        service.refresh_keys(None).await.unwrap();
        let view = service.session_view(OrangeProxy::Direct).await.unwrap();

        assert_eq!(
            view.warning.as_deref(),
            Some(OrangeError::CredentialStore.code())
        );
        assert!(!view.remembered);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn transient_restore_failure_keeps_the_saved_refresh_token_for_retry() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(POST).path("/api/v1/auth/refresh");
                then.status(503);
            })
            .await;

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("orange-restore-transient-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let token_store = Arc::new(MemoryTokenStore(StdMutex::new(Some("refresh".into()))));
        let service = OrangeSessionService::for_test(
            root.join("session.json"),
            token_store.clone(),
            SessionMetadata {
                last_email: Some("a@b.test".into()),
                remember_login: true,
                site_name: None,
            },
        );
        {
            let mut state = service.state.lock().await;
            state.proxy = Some(OrangeProxy::Direct);
            state.client = Some(OrangeApiClient::for_test(server.base_url()));
        }

        let view = service.session_view(OrangeProxy::Direct).await.unwrap();

        assert!(!view.authenticated);
        assert!(view.remembered);
        assert_eq!(view.warning.as_deref(), Some("orange_api_rejected"));
        assert_eq!(token_store.load().unwrap().as_deref(), Some("refresh"));
        assert!(!service.state.lock().await.restore_attempted);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn refreshes_once_then_loads_all_openai_pages_and_marks_the_local_key() {
        let server = MockServer::start_async().await;
        let unauthorized = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/api/v1/keys")
                    .header("authorization", "Bearer expired-access");
                then.status(401);
            })
            .await;
        let refresh = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/api/v1/auth/refresh")
                    .json_body_obj(&serde_json::json!({"refresh_token":"old-refresh"}));
                then.status(200).json_body_obj(&serde_json::json!({
                    "code": 0,
                    "data": {
                        "access_token": "new-access",
                        "refresh_token": "new-refresh",
                        "expires_in": 3600
                    }
                }));
            })
            .await;
        let page_one = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/api/v1/keys")
                    .header("authorization", "Bearer new-access")
                    .query_param("page", "1");
                then.status(200).json_body_obj(&serde_json::json!({
                    "code": 0,
                    "data": {
                        "items": [
                            {
                                "id": 1,
                                "key": "sk-one",
                                "name": "One",
                                "status": "active",
                                "quota": 0,
                                "quota_used": 0,
                                "expires_at": null,
                                "created_at": "2026-01-01T00:00:00Z",
                                "group": {"name":"OpenAI","platform":"openai","status":"active"}
                            },
                            {
                                "id": 2,
                                "key": "sk-anthropic",
                                "name": "Other",
                                "status": "active",
                                "quota": 0,
                                "quota_used": 0,
                                "expires_at": null,
                                "created_at": "2026-02-01T00:00:00Z",
                                "group": {"name":"Claude","platform":"anthropic","status":"active"}
                            }
                        ],
                        "total": 3,
                        "page": 1,
                        "page_size": 50,
                        "pages": 2
                    }
                }));
            })
            .await;
        let page_two = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/api/v1/keys")
                    .header("authorization", "Bearer new-access")
                    .query_param("page", "2");
                then.status(200).json_body_obj(&serde_json::json!({
                    "code": 0,
                    "data": {
                        "items": [{
                            "id": 3,
                            "key": "sk-three",
                            "name": "Three",
                            "status": "active",
                            "quota": 0,
                            "quota_used": 0,
                            "expires_at": null,
                            "created_at": "2026-03-01T00:00:00Z",
                            "group": {"name":"OpenAI","platform":"openai","status":"active"}
                        }],
                        "total": 3,
                        "page": 2,
                        "page_size": 50,
                        "pages": 2
                    }
                }));
            })
            .await;

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("orange-key-pages-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let token_store = Arc::new(MemoryTokenStore(StdMutex::new(Some("old-refresh".into()))));
        let service = OrangeSessionService::for_test(
            root.join("session.json"),
            token_store.clone(),
            SessionMetadata {
                last_email: Some("a@b.test".into()),
                remember_login: true,
                site_name: None,
            },
        );
        {
            let mut state = service.state.lock().await;
            state.proxy = Some(OrangeProxy::Direct);
            state.client = Some(OrangeApiClient::for_test(server.base_url()));
            state.access_token = Some("expired-access".into());
            state.refresh_token = Some("old-refresh".into());
        }

        let keys = service.refresh_keys(Some("sk-one")).await.unwrap();

        assert_eq!(
            keys.items.iter().map(|key| key.id).collect::<Vec<_>>(),
            vec![3, 1]
        );
        assert!(keys.items.iter().find(|key| key.id == 1).unwrap().enabled);
        assert_eq!(token_store.load().unwrap().as_deref(), Some("new-refresh"));
        unauthorized.assert_hits_async(1).await;
        refresh.assert_hits_async(1).await;
        page_one.assert_hits_async(1).await;
        page_two.assert_hits_async(1).await;
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn login_remains_connected_when_metadata_cannot_be_persisted() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/api/v1/settings/public");
                then.status(200).json_body_obj(&serde_json::json!({
                    "code": 0,
                    "data": {"turnstile_enabled": false, "site_name": "Cylon API"}
                }));
            })
            .await;
        server
            .mock_async(|when, then| {
                when.method(POST).path("/api/v1/auth/login");
                then.status(200).json_body_obj(&serde_json::json!({
                    "code": 0,
                    "data": {
                        "access_token": "access",
                        "refresh_token": "refresh",
                        "expires_in": 3600,
                        "user": {"email": "a@b.test"}
                    }
                }));
            })
            .await;

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("orange-login-persistence-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let blocked_parent = root.join("not-a-directory");
        fs::write(&blocked_parent, b"blocked").unwrap();
        let token_store = Arc::new(MemoryTokenStore::default());
        let service = OrangeSessionService::for_test(
            blocked_parent.join("session.json"),
            token_store.clone(),
            SessionMetadata::default(),
        );
        {
            let mut state = service.state.lock().await;
            state.proxy = Some(OrangeProxy::Direct);
            state.client = Some(OrangeApiClient::for_test(server.base_url()));
        }

        let view = service
            .login(OrangeProxy::Direct, "a@b.test".into(), "pw".into(), true)
            .await
            .unwrap();

        assert!(view.authenticated);
        assert!(!view.remembered);
        assert_eq!(view.warning.as_deref(), Some("orange_persistence"));
        assert_eq!(service.provider_name().await, "Cylon API");
        assert!(token_store.load().unwrap().is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn logout_remains_signed_out_when_metadata_cannot_be_persisted() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!(
                "orange-logout-persistence-{}",
                uuid::Uuid::new_v4()
            ));
        fs::create_dir_all(&root).unwrap();
        let blocked_parent = root.join("not-a-directory");
        fs::write(&blocked_parent, b"blocked").unwrap();
        let token_store = Arc::new(MemoryTokenStore(StdMutex::new(Some("refresh".into()))));
        let service = OrangeSessionService::for_test(
            blocked_parent.join("session.json"),
            token_store.clone(),
            SessionMetadata {
                last_email: Some("a@b.test".into()),
                remember_login: true,
                site_name: None,
            },
        );
        {
            let mut state = service.state.lock().await;
            state.access_token = Some("access".into());
            state.refresh_token = Some("refresh".into());
            state.email = Some("a@b.test".into());
        }

        let view = service.logout().await.unwrap();

        assert!(!view.authenticated);
        assert!(!view.remembered);
        assert_eq!(view.email.as_deref(), Some("a@b.test"));
        assert_eq!(view.warning.as_deref(), Some("orange_persistence"));
        assert!(token_store.load().unwrap().is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn metadata_never_serializes_a_password_or_token() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("orange-session-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session.json");
        let metadata = SessionMetadata {
            last_email: Some("a@b.test".into()),
            remember_login: true,
            site_name: Some("Cylon API".into()),
        };
        let service = OrangeSessionService::for_test(
            path.clone(),
            Arc::new(MemoryTokenStore::default()),
            metadata.clone(),
        );
        service.set_remembered(true).await.unwrap();
        let serialized = fs::read_to_string(path).unwrap();
        assert!(serialized.contains("a@b.test"));
        assert!(serialized.contains("Cylon API"));
        assert!(!serialized.contains("password"));
        assert!(!serialized.contains("token"));
        let _ = fs::remove_dir_all(root);
    }
}
