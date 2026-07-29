use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const ORANGE_BASE_URL: &str = "https://token.cylonai.cn";
pub const ORANGE_API_PREFIX: &str = "/api/v1";
const USER_AGENT: &str = "CodexAppManager/0.5 OrangeAPI";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrangeProxy {
    System,
    Direct,
    Custom(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OrangeError {
    #[error("OrangeAPI authentication failed")]
    Unauthorized,
    #[error("OrangeAPI request was forbidden")]
    Forbidden,
    #[error("OrangeAPI request was rate limited")]
    RateLimited,
    #[error("OrangeAPI request timed out")]
    Timeout,
    #[error("OrangeAPI is unreachable")]
    Network,
    #[error("OrangeAPI returned an invalid response")]
    InvalidResponse,
    #[error("OrangeAPI rejected the request")]
    ApiRejected,
    #[error("OrangeAPI requires two-step verification")]
    TwoFactorUnsupported,
    #[error("OrangeAPI requires Turnstile verification")]
    TurnstileUnsupported,
    #[error("OrangeAPI credentials are unavailable")]
    CredentialStore,
    #[error("OrangeAPI session is signed out")]
    SignedOut,
    #[error("OrangeAPI key is unavailable")]
    KeyUnavailable,
    #[error("OrangeAPI configuration could not be persisted")]
    Persistence,
    #[error("this operation is not supported on the current platform")]
    UnsupportedPlatform,
}

impl OrangeError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unauthorized => "orange_invalid_credentials",
            Self::Forbidden => "orange_forbidden",
            Self::RateLimited => "orange_rate_limited",
            Self::Timeout => "orange_timeout",
            Self::Network => "orange_network",
            Self::InvalidResponse => "orange_invalid_response",
            Self::ApiRejected => "orange_api_rejected",
            Self::TwoFactorUnsupported => "orange_2fa_unsupported",
            Self::TurnstileUnsupported => "orange_turnstile_unsupported",
            Self::CredentialStore => "orange_credential_store",
            Self::SignedOut => "orange_signed_out",
            Self::KeyUnavailable => "orange_key_unavailable",
            Self::Persistence => "orange_persistence",
            Self::UnsupportedPlatform => "unsupported_platform",
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope<T> {
    code: i64,
    #[allow(dead_code)]
    message: Option<String>,
    data: Option<T>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PublicSettings {
    #[serde(default)]
    pub turnstile_enabled: bool,
    #[serde(default)]
    pub site_name: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct OrangeUser {
    pub email: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    #[allow(dead_code)]
    pub token_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    #[allow(dead_code)]
    pub token_type: Option<String>,
    pub user: OrangeUser,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    Authenticated(AuthTokens),
    RequiresTwoFactor,
}

#[derive(Debug, Deserialize)]
struct LoginData {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    token_type: Option<String>,
    user: Option<OrangeUser>,
    #[serde(default)]
    requires_2fa: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawGroup {
    pub name: String,
    pub platform: String,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawApiKey {
    pub id: u64,
    pub key: String,
    pub name: String,
    pub status: String,
    pub quota: f64,
    pub quota_used: f64,
    pub expires_at: Option<String>,
    pub created_at: String,
    pub group: Option<RawGroup>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KeyPage {
    #[serde(default)]
    pub items: Vec<RawApiKey>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub pages: u32,
}

#[derive(Clone)]
pub struct OrangeApiClient {
    base_url: String,
    client: Client,
}

impl OrangeApiClient {
    pub fn new(proxy: OrangeProxy) -> Result<Self, OrangeError> {
        Self::build(ORANGE_BASE_URL.to_string(), proxy)
    }

    #[cfg(test)]
    pub fn for_test(base_url: String) -> Self {
        Self::build(base_url, OrangeProxy::Direct).expect("test client")
    }

    fn build(base_url: String, proxy: OrangeProxy) -> Result<Self, OrangeError> {
        let mut builder = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .user_agent(USER_AGENT);
        match proxy {
            OrangeProxy::System => {}
            OrangeProxy::Direct => builder = builder.no_proxy(),
            OrangeProxy::Custom(url) => {
                let proxy = reqwest::Proxy::all(url).map_err(|_| OrangeError::Network)?;
                builder = builder.no_proxy().proxy(proxy);
            }
        }
        let client = builder.build().map_err(|_| OrangeError::Network)?;
        Ok(Self { base_url, client })
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}{}{}",
            self.base_url.trim_end_matches('/'),
            ORANGE_API_PREFIX,
            path
        )
    }

    async fn decode<T: DeserializeOwned>(
        &self,
        response: Result<reqwest::Response, reqwest::Error>,
    ) -> Result<T, OrangeError> {
        let response = response.map_err(classify_transport)?;
        match response.status() {
            StatusCode::UNAUTHORIZED => return Err(OrangeError::Unauthorized),
            StatusCode::FORBIDDEN => return Err(OrangeError::Forbidden),
            StatusCode::TOO_MANY_REQUESTS => return Err(OrangeError::RateLimited),
            status if !status.is_success() => return Err(OrangeError::ApiRejected),
            _ => {}
        }
        let envelope: ApiEnvelope<T> = response
            .json()
            .await
            .map_err(|_| OrangeError::InvalidResponse)?;
        if envelope.code != 0 {
            return Err(OrangeError::ApiRejected);
        }
        envelope.data.ok_or(OrangeError::InvalidResponse)
    }

    pub async fn public_settings(&self) -> Result<PublicSettings, OrangeError> {
        self.decode(self.client.get(self.url("/settings/public")).send().await)
            .await
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<LoginOutcome, OrangeError> {
        #[derive(Serialize)]
        struct LoginRequest<'a> {
            email: &'a str,
            password: &'a str,
        }
        let data: LoginData = self
            .decode(
                self.client
                    .post(self.url("/auth/login"))
                    .json(&LoginRequest { email, password })
                    .send()
                    .await,
            )
            .await?;
        if data.requires_2fa {
            return Ok(LoginOutcome::RequiresTwoFactor);
        }
        let (Some(access_token), Some(user)) = (data.access_token, data.user) else {
            return Err(OrangeError::InvalidResponse);
        };
        Ok(LoginOutcome::Authenticated(AuthTokens {
            access_token,
            refresh_token: data.refresh_token,
            expires_in: data.expires_in,
            token_type: data.token_type,
            user,
        }))
    }

    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenPair, OrangeError> {
        #[derive(Serialize)]
        struct RefreshRequest<'a> {
            refresh_token: &'a str,
        }
        self.decode(
            self.client
                .post(self.url("/auth/refresh"))
                .json(&RefreshRequest { refresh_token })
                .send()
                .await,
        )
        .await
    }

    pub async fn logout(&self, refresh_token: &str) -> Result<(), OrangeError> {
        #[derive(Serialize)]
        struct LogoutRequest<'a> {
            refresh_token: &'a str,
        }
        let _: serde_json::Value = self
            .decode(
                self.client
                    .post(self.url("/auth/logout"))
                    .json(&LogoutRequest { refresh_token })
                    .send()
                    .await,
            )
            .await?;
        Ok(())
    }

    pub async fn key_page(&self, access_token: &str, page: u32) -> Result<KeyPage, OrangeError> {
        self.decode(
            self.client
                .get(self.url("/keys"))
                .bearer_auth(access_token)
                .query(&[
                    ("page", page.to_string()),
                    ("page_size", "50".into()),
                    ("sort_by", "created_at".into()),
                    ("sort_order", "desc".into()),
                ])
                .send()
                .await,
        )
        .await
    }
}

fn classify_transport(error: reqwest::Error) -> OrangeError {
    if error.is_timeout() {
        OrangeError::Timeout
    } else if error.is_connect() {
        OrangeError::Network
    } else {
        OrangeError::InvalidResponse
    }
}

#[cfg(test)]
mod tests {
    use httpmock::prelude::*;

    use super::*;

    #[tokio::test]
    async fn login_posts_expected_body_and_unwraps_envelope() {
        let server = MockServer::start_async().await;
        let login = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/api/v1/auth/login")
                    .json_body_obj(&serde_json::json!({"email":"a@b.test","password":"pw"}));
                then.status(200).json_body_obj(&serde_json::json!({
                    "code": 0,
                    "message": "success",
                    "data": {
                        "access_token": "access",
                        "refresh_token": "refresh",
                        "expires_in": 3600,
                        "token_type": "Bearer",
                        "user": {"email":"a@b.test"}
                    }
                }));
            })
            .await;
        let client = OrangeApiClient::for_test(server.base_url());
        let result = client.login("a@b.test", "pw").await.unwrap();
        let LoginOutcome::Authenticated(tokens) = result else {
            panic!("expected tokens")
        };
        assert_eq!(tokens.refresh_token.as_deref(), Some("refresh"));
        login.assert_async().await;
    }

    #[tokio::test]
    async fn public_settings_and_two_factor_login_follow_the_service_contract() {
        let server = MockServer::start_async().await;
        let settings = server
            .mock_async(|when, then| {
                when.method(GET).path("/api/v1/settings/public");
                then.status(200).json_body_obj(&serde_json::json!({
                    "code": 0,
                    "message": "success",
                    "data": {"turnstile_enabled": true, "site_name": "Cylon API"}
                }));
            })
            .await;
        let login = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/api/v1/auth/login")
                    .json_body_obj(&serde_json::json!({"email":"a@b.test","password":"pw"}));
                then.status(200).json_body_obj(&serde_json::json!({
                    "code": 0,
                    "message": "success",
                    "data": {"requires_2fa": true}
                }));
            })
            .await;
        let client = OrangeApiClient::for_test(server.base_url());

        let public = client.public_settings().await.unwrap();
        assert!(public.turnstile_enabled);
        assert_eq!(public.site_name, "Cylon API");
        assert_eq!(
            client.login("a@b.test", "pw").await.unwrap(),
            LoginOutcome::RequiresTwoFactor
        );
        settings.assert_async().await;
        login.assert_async().await;
    }

    #[tokio::test]
    async fn refresh_and_logout_post_the_refresh_token() {
        let server = MockServer::start_async().await;
        let refresh = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/api/v1/auth/refresh")
                    .json_body_obj(&serde_json::json!({"refresh_token":"old-refresh"}));
                then.status(200).json_body_obj(&serde_json::json!({
                    "code": 0,
                    "message": "success",
                    "data": {
                        "access_token": "new-access",
                        "refresh_token": "new-refresh",
                        "expires_in": 3600,
                        "token_type": "Bearer"
                    }
                }));
            })
            .await;
        let logout = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/api/v1/auth/logout")
                    .json_body_obj(&serde_json::json!({"refresh_token":"new-refresh"}));
                then.status(200).json_body_obj(&serde_json::json!({
                    "code": 0,
                    "message": "success",
                    "data": {"message": "Logged out successfully"}
                }));
            })
            .await;
        let client = OrangeApiClient::for_test(server.base_url());

        let tokens = client.refresh("old-refresh").await.unwrap();
        assert_eq!(tokens.access_token, "new-access");
        assert_eq!(tokens.refresh_token, "new-refresh");
        client.logout(&tokens.refresh_token).await.unwrap();
        refresh.assert_async().await;
        logout.assert_async().await;
    }

    #[tokio::test]
    async fn key_page_uses_the_orangeapi_sort_contract() {
        let server = MockServer::start_async().await;
        let request = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/api/v1/keys")
                    .header("authorization", "Bearer access")
                    .query_param("page", "2")
                    .query_param("page_size", "50")
                    .query_param("sort_by", "created_at")
                    .query_param("sort_order", "desc");
                then.status(200).json_body_obj(&serde_json::json!({
                    "code": 0,
                    "data": {"items": [], "total": 0, "page": 2, "page_size": 50, "pages": 2}
                }));
            })
            .await;
        let client = OrangeApiClient::for_test(server.base_url());
        client.key_page("access", 2).await.unwrap();
        request.assert_async().await;
    }

    #[tokio::test]
    async fn errors_are_classified_without_echoing_response_bodies() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/api/v1/settings/public");
                then.status(429).body("sk-secret-must-not-escape");
            })
            .await;
        let error = OrangeApiClient::for_test(server.base_url())
            .public_settings()
            .await
            .unwrap_err();
        assert_eq!(error, OrangeError::RateLimited);
        assert!(!error.to_string().contains("sk-secret"));
    }

    #[tokio::test]
    async fn rejects_nonzero_and_malformed_success_envelopes() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/api/v1/settings/public");
                then.status(200).json_body_obj(&serde_json::json!({
                    "code": 123,
                    "message": "rejected",
                    "data": {"turnstile_enabled": false}
                }));
            })
            .await;
        let client = OrangeApiClient::for_test(server.base_url());
        assert_eq!(
            client.public_settings().await.unwrap_err(),
            OrangeError::ApiRejected
        );

        let malformed_server = MockServer::start_async().await;
        malformed_server
            .mock_async(|when, then| {
                when.method(GET).path("/api/v1/settings/public");
                then.status(200).body("not-json");
            })
            .await;
        assert_eq!(
            OrangeApiClient::for_test(malformed_server.base_url())
                .public_settings()
                .await
                .unwrap_err(),
            OrangeError::InvalidResponse
        );
    }
}
