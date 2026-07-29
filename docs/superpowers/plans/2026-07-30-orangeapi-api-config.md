# OrangeAPI API Configuration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a secure API Configuration view that logs into the fixed OrangeAPI service, lists OpenAI keys, imports them into CC Switch, and transactionally installs one into local Codex on Windows and macOS.

**Architecture:** Rust owns the HTTP client, session, secrets, full key cache, CC Switch URI, Codex process coordination, and two-file write transaction. React receives masked presentation models over guarded Tauri IPC and implements the signed-out/signed-in workflow using the existing navigation, sheet, banner, and list patterns. OrangeAPI is consumed through its existing `/api/v1` endpoints and is never modified.

**Tech Stack:** Tauri v2, Rust 2021, reqwest 0.13, keyring 4.1, serde/serde_json, React 19, TypeScript 6, Vitest/Testing Library, existing Codex macOS/Windows engines.

---

## File Structure

### New files

- `src-tauri/src/app/orange_api.rs` - fixed service configuration, HTTP transport, envelope parsing, proxy handling, auth calls, and paginated key retrieval.
- `src-tauri/src/app/orange_session.rs` - in-memory session, refresh-token credential abstraction, last-email metadata, key cache, masking/actionability, and enabled-key projection.
- `src-tauri/src/app/codex_provider.rs` - CC Switch deep link, local Codex templates, durable backup manifest, two-file commit/rollback, and enabled-key inspection.
- `src/app/views/apiConfig/ApiLoginForm.tsx` - signed-out login form only.
- `src/app/views/apiConfig/ApiKeyList.tsx` - signed-in connection header, key rows, pagination, actions, and logout.
- `src/app/views/apiConfig/ApiLoginForm.test.tsx` - login-state behavior and accessibility tests.
- `src/app/views/apiConfig/ApiKeyList.test.tsx` - list/action/pagination/enabled-state tests.
- `src/app/App.test.tsx` - top-level config-route and focus behavior tests.
- `src/app/Rail.test.tsx` - expanded-rail order and active-section tests.
- `src/app/views/CodexConfig.test.tsx` - complete signed-out/signed-in orchestration tests.

### Modified files

- `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock` - reqwest JSON/query/TLS, platform keyring backends, and HTTP mock test dependency.
- `src-tauri/src/app/mod.rs` - register the three backend modules.
- `src-tauri/src/app/paths.rs` - Orange session metadata and backup-root paths.
- `src-tauri/src/app/codex_theme.rs` - expose the existing process-safe stop/settle and plain restart primitives.
- `src-tauri/src/state.rs` - own one `OrangeSessionService`.
- `src-tauri/src/commands.rs` - add API Configuration Tauri commands and map stable integration errors.
- `src-tauri/src/lib.rs` - register the commands.
- `src/shared/types.ts` - frontend-safe session/key/write report contracts.
- `src/services/managerApi.ts` and `src/services/managerApi.test.ts` - guarded IPC methods and invocation tests.
- `src/app/App.tsx` and its tests - make config a first-class route/rail section.
- `src/app/Rail.tsx` and its tests - add API Configuration below Home.
- `src/app/views/Settings.tsx` and `Settings.test.tsx` - enable the compact-mode config entry.
- `src/app/views/CodexConfig.tsx` and `CodexConfig.test.tsx` - replace the placeholder with the orchestration view.
- `src/app/icons.tsx` - add key, eye/eye-off, logout, and link icons in the existing stroke system.
- `src/app/i18n.tsx` and `src/app/i18n.test.tsx` - add complete copy in all 11 supported locales.
- `src/app/styles.css` - scoped API Configuration layout, fixed action sizing, narrow/expanded behavior, and RTL rules.

## Task 1: Backend Contracts and Dependencies

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/Cargo.lock`
- Modify: `src-tauri/src/app/mod.rs`
- Create: `src-tauri/src/app/orange_api.rs`
- Create: `src-tauri/src/app/orange_session.rs`
- Create: `src-tauri/src/app/codex_provider.rs`

- [ ] **Step 1: Write failing domain tests for key actionability and masking**

Add tests to `orange_session.rs` before defining the production types:

```rust
#[cfg(test)]
mod tests {
    use super::{mask_api_key, RawApiKey};

    fn key(status: &str, group_status: &str, quota: f64, used: f64) -> RawApiKey {
        RawApiKey::fixture(status, group_status, quota, used, None)
    }

    #[test]
    fn masks_without_leaking_short_or_long_keys() {
        assert_eq!(mask_api_key("sk-1234567890"), "sk-1••••••7890");
        assert_eq!(mask_api_key("short"), "•••••");
    }

    #[test]
    fn actionability_requires_active_group_unexpired_key_and_quota() {
        assert!(key("active", "active", 0.0, 99.0).actionable_at(1_000));
        assert!(!key("inactive", "active", 0.0, 0.0).actionable_at(1_000));
        assert!(!key("active", "inactive", 0.0, 0.0).actionable_at(1_000));
        assert!(!key("active", "active", 10.0, 10.0).actionable_at(1_000));
    }
}
```

- [ ] **Step 2: Run the focused test and verify the expected compile failure**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml orange_session::tests -- --nocapture
```

Expected: compilation fails because `RawApiKey`, `fixture`, `actionable_at`, and `mask_api_key` do not exist.

- [ ] **Step 3: Add dependencies and the minimal shared contracts**

Use these dependency shapes:

```toml
reqwest = { version = "0.13.4", default-features = false, features = ["json", "query", "rustls", "socks", "system-proxy"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "sync", "time"] }

[target.'cfg(target_os = "macos")'.dependencies]
keyring = { version = "4.1.5", default-features = false, features = ["apple-native-keyring-store"] }

[target.'cfg(target_os = "windows")'.dependencies]
keyring = { version = "4.1.5", default-features = false, features = ["windows-native-keyring-store"] }

[dev-dependencies]
httpmock = "0.8.3"
```

Register the new modules in `app/mod.rs`. Define the raw API shapes in
`orange_api.rs` and the frontend-safe shapes in `orange_session.rs` with
`#[serde(rename_all = "camelCase")]`. Use exact status strings
`active|inactive|quota_exhausted|expired`. `actionable_at` must require active
key and group, future/no expiry, and unlimited or remaining quota. Mask values
without logging or cloning them into presentation objects.

- [ ] **Step 4: Run formatting and focused tests**

Run:

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml orange_session::tests -- --nocapture
```

Expected: both masking/actionability tests pass and Cargo.lock contains only dependency resolution changes.

- [ ] **Step 5: Commit the contract slice**

```powershell
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/app/mod.rs src-tauri/src/app/orange_api.rs src-tauri/src/app/orange_session.rs src-tauri/src/app/codex_provider.rs
git commit -m "feat: add OrangeAPI integration contracts"
```

## Task 2: OrangeAPI HTTP Client

**Files:**
- Modify: `src-tauri/src/app/orange_api.rs`

- [ ] **Step 1: Write failing HTTP contract tests against a local mock server**

Cover the exact envelope and endpoints:

```rust
#[tokio::test]
async fn login_posts_expected_body_and_unwraps_envelope() {
    let server = MockServer::start_async().await;
    let login = server.mock_async(|when, then| {
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
    }).await;
    let client = OrangeApiClient::for_test(server.base_url());
    let result = client.login("a@b.test", "pw").await.unwrap();
    assert_eq!(result.refresh_token.as_deref(), Some("refresh"));
    login.assert_async().await;
}
```

Add tests for public settings, refresh rotation, logout body, 429 mapping,
`requires_2fa`, malformed envelopes, and `GET /keys` query parameters.

- [ ] **Step 2: Run tests and verify failure**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml orange_api::tests -- --nocapture
```

Expected: FAIL because `OrangeApiClient` and its endpoint methods are not implemented.

- [ ] **Step 3: Implement the fixed, proxy-aware client**

Define these public boundaries:

```rust
pub const ORANGE_BASE_URL: &str = "https://token.cylonai.cn";
pub const ORANGE_API_PREFIX: &str = "/api/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrangeProxy {
    System,
    Direct,
    Custom(String),
}

#[derive(Clone)]
pub struct OrangeApiClient {
    base_url: String,
    client: reqwest::Client,
}

impl OrangeApiClient {
    pub fn new(proxy: OrangeProxy) -> Result<Self, OrangeError>;
    pub async fn public_settings(&self) -> Result<PublicSettings, OrangeError>;
    pub async fn login(&self, email: &str, password: &str) -> Result<LoginOutcome, OrangeError>;
    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenPair, OrangeError>;
    pub async fn logout(&self, refresh_token: &str) -> Result<(), OrangeError>;
    pub async fn key_page(&self, access_token: &str, page: u32) -> Result<KeyPage, OrangeError>;
}
```

Use a 10-second connect timeout, 30-second request timeout, stable User-Agent,
`.no_proxy()` for Direct, and `.no_proxy().proxy(Proxy::all(...))` for Custom.
Deserialize every success through `ApiEnvelope<T>`. Convert status 401, 403,
429, timeout, connect failure, malformed JSON, API `code != 0`, and 2FA into
stable `OrangeError::code()` values without including response bodies that may
contain secrets.

- [ ] **Step 4: Run the full client test set**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml orange_api::tests -- --nocapture
```

Expected: all endpoint, envelope, proxy, timeout classification, and redaction tests pass.

- [ ] **Step 5: Commit the client**

```powershell
git add src-tauri/src/app/orange_api.rs
git commit -m "feat: implement OrangeAPI client"
```

## Task 3: Credential Store and Session Restoration

**Files:**
- Modify: `src-tauri/src/app/paths.rs`
- Modify: `src-tauri/src/app/orange_session.rs`
- Modify: `src-tauri/src/state.rs`

- [ ] **Step 1: Write failing tests with an in-memory credential store**

Use a trait so tests never touch the real keychain:

```rust
trait RefreshTokenStore: Send + Sync {
    fn load(&self) -> Result<Option<String>, OrangeError>;
    fn save(&self, token: &str) -> Result<(), OrangeError>;
    fn clear(&self) -> Result<(), OrangeError>;
}

#[tokio::test]
async fn remembered_login_rotates_secret_and_retains_email_on_logout() {
    let store = Arc::new(MemoryTokenStore::with("old-refresh"));
    let service = test_service(store.clone(), "rotated-refresh");
    let view = service.restore().await.unwrap();
    assert!(view.authenticated);
    assert_eq!(store.value(), Some("rotated-refresh".into()));
    service.logout().await.unwrap();
    assert_eq!(store.value(), None);
    assert_eq!(service.metadata().last_email.as_deref(), Some("a@b.test"));
}
```

Also test no saved token, unremembered login clearing an old token, credential
save failure warning, refresh failure clearing the secret, and password absence
from serialized metadata.

- [ ] **Step 2: Run session tests and verify failure**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml orange_session::tests -- --nocapture
```

Expected: FAIL because the credential abstraction and `OrangeSessionService` do not exist.

- [ ] **Step 3: Implement metadata, OS credentials, and session state**

Add paths:

```rust
pub fn orange_session_path() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("orangeapi-session.json"))
}

pub fn orange_backup_root() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("orangeapi-backups"))
}
```

Use service `io.github.wangnov.codexappmanager` and username
`orangeapi-refresh-token` for `keyring::Entry`. Treat `keyring::Error::NoEntry`
as no saved token. Persist only this metadata atomically:

```rust
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionMetadata {
    last_email: Option<String>,
    remember_login: bool,
}
```

`OrangeSessionService` owns `tokio::sync::Mutex<SessionState>`, one current
proxy/client fingerprint, `Arc<dyn RefreshTokenStore>`, and the metadata path.
Store access token, rotated refresh token, expiry instant, user email, last
connection result, and complete key cache only inside `SessionState`. Add it to
`ManagerState::new()` as `pub orange: OrangeSessionService`. Manual login first
reads public settings and returns `orange_turnstile_unsupported` when Turnstile
is enabled. It then calls login and returns `orange_2fa_unsupported` for a
`requires_2fa` outcome; only a complete token response mutates session state.

- [ ] **Step 4: Run session and state tests**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml orange_session::tests state::tests -- --nocapture
```

Expected: remember/restore/rotation/logout tests pass and no real credential entry is created.

- [ ] **Step 5: Commit the session slice**

```powershell
git add src-tauri/src/app/paths.rs src-tauri/src/app/orange_session.rs src-tauri/src/state.rs
git commit -m "feat: persist OrangeAPI sessions securely"
```

## Task 4: Token Refresh, Key Paging, Cache, and Enabled Projection

**Files:**
- Modify: `src-tauri/src/app/orange_session.rs`
- Modify: `src-tauri/src/app/orange_api.rs`

- [ ] **Step 1: Write failing session-flow tests**

Add mock-server tests proving:

```rust
#[tokio::test]
async fn keys_refresh_once_on_401_then_filters_all_openai_pages() {
    // Page 1 returns OpenAI + Anthropic, page 2 returns OpenAI.
    // The first key request returns 401, refresh rotates the token, and the
    // retry plus page 2 use only the new access token.
    let keys = service.refresh_keys(Some("sk-enabled")).await.unwrap();
    assert_eq!(keys.iter().map(|key| key.id).collect::<Vec<_>>(), vec![3, 1]);
    assert!(keys.iter().find(|key| key.id == 1).unwrap().enabled);
    assert_eq!(refresh_mock.hits_async().await, 1);
}
```

Also test a second 401 ending the session, `pages=0`, malformed pagination,
missing group, inactive group, expired date, stale-cache ID lookup, and a list
failure preserving the previous projected list as stale.

- [ ] **Step 2: Run and verify failures**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml orange_session::tests -- --nocapture
```

Expected: new flow tests fail because refresh/retry/paging/cache projection is absent.

- [ ] **Step 3: Implement the authenticated request state machine**

Implement these service methods:

```rust
pub async fn session_view(&self, proxy: OrangeProxy) -> Result<OrangeSessionView, OrangeError>;
pub async fn login(&self, proxy: OrangeProxy, email: String, password: String, remember: bool)
    -> Result<OrangeLoginReport, OrangeError>;
pub async fn refresh_keys(&self, local_key: Option<&str>)
    -> Result<OrangeKeyList, OrangeError>;
pub async fn logout(&self) -> Result<OrangeSessionView, OrangeError>;
pub async fn full_key(&self, id: u64) -> Result<String, OrangeError>;
```

Refresh 30 seconds before expiry. Retry only once after 401. Follow pages with
`page_size=50&sort_by=created_at&sort_order=desc`, cap at the declared `pages`,
reject a page number that does not advance, filter
`group.platform == "openai"`, and sort by parsed `created_at` descending with ID
as a deterministic tie-breaker. Preserve but mark old
presentation data stale on transport failure. Clear all tokens/cache after the
terminal 401 but retain metadata email.

- [ ] **Step 4: Run flow and redaction tests**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml orange_session::tests -- --nocapture
```

Expected: all paging, filtering, retry, expiry, stale-cache, enabled, and secret-redaction tests pass.

- [ ] **Step 5: Commit the authenticated flow**

```powershell
git add src-tauri/src/app/orange_api.rs src-tauri/src/app/orange_session.rs
git commit -m "feat: load and cache OpenAI API keys"
```

## Task 5: CC Switch Import

**Files:**
- Modify: `src-tauri/src/app/codex_provider.rs`
- Modify: `src-tauri/src/commands.rs`

- [ ] **Step 1: Write a failing exact deep-link test**

```rust
#[test]
fn ccs_link_matches_orangeapi_codex_contract() {
    let uri = build_ccswitch_uri("酸橘子", "sk-a+b&c");
    let parsed = url::Url::parse(&uri).unwrap();
    let query = parsed.query_pairs().into_owned().collect::<HashMap<_, _>>();
    assert_eq!(parsed.scheme(), "ccswitch");
    assert_eq!(parsed.host_str(), Some("v1"));
    assert_eq!(parsed.path(), "/import");
    assert_eq!(query["resource"], "provider");
    assert_eq!(query["app"], "codex");
    assert_eq!(query["model"], "gpt-5.5");
    assert_eq!(query["endpoint"], "https://token.cylonai.cn");
    assert_eq!(query["apiKey"], "sk-a+b&c");
    let script = STANDARD.decode(&query["usageScript"]).unwrap();
    assert!(String::from_utf8(script).unwrap().contains("{{baseUrl}}/v1/usage"));
}
```

- [ ] **Step 2: Run and verify failure**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml codex_provider::tests::ccs_link -- --nocapture
```

Expected: FAIL because `build_ccswitch_uri` is missing.

- [ ] **Step 3: Implement internal-only URI construction and platform launch**

Build query pairs with `url::form_urlencoded::Serializer`; Base64-encode the
exact usage script from OrangeAPI's `KeysView.vue`. Never log the URI. Add a
private `open_ccswitch_uri` that uses `ShellExecuteW` on Windows and
`/usr/bin/open <uri>` with checked exit status on macOS. The Tauri command
accepts only `key_id: u64`, resolves the full key from `ManagerState.orange`,
constructs the URI internally, and returns an unsupported-platform error
elsewhere. Do not weaken the existing HTTP-only `open_url` validator.

- [ ] **Step 4: Run URI and platform compilation tests**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml codex_provider::tests -- --nocapture
cargo check --manifest-path src-tauri/Cargo.toml
```

Expected: URI parsing/encoding tests pass; the current platform compiles without exposing a generic protocol command.

- [ ] **Step 5: Commit CC Switch support**

```powershell
git add src-tauri/src/app/codex_provider.rs src-tauri/src/commands.rs
git commit -m "feat: import OrangeAPI keys into CC Switch"
```

## Task 6: Durable Two-File Codex Transaction

**Files:**
- Modify: `src-tauri/src/app/codex_provider.rs`
- Modify: `src-tauri/src/app/atomic_file.rs`

- [ ] **Step 1: Write failing transaction and rollback tests in isolated roots**

Use explicit temporary directories under `src-tauri/target/test-data`; never use
the real home:

```rust
#[test]
fn replaces_both_files_after_durable_backup_and_verification() {
    let fixture = ProviderFixture::with_existing("old-config", r#"{"OPENAI_API_KEY":"old"}"#);
    let report = fixture.write("sk-new", ProviderWriteFault::None).unwrap();
    assert_eq!(fs::read_to_string(fixture.config()).unwrap(), expected_config());
    assert_eq!(read_auth_key(&fixture.auth()).unwrap().as_deref(), Some("sk-new"));
    assert_eq!(fs::read_to_string(report.backup_dir.join("config.toml")).unwrap(), "old-config");
    assert!(verify_manifest(&report.backup_dir.join("manifest.json")));
}

#[test]
fn second_replace_failure_restores_both_preimages() {
    let fixture = ProviderFixture::with_existing("old-config", r#"{"OPENAI_API_KEY":"old"}"#);
    let report = fixture.write("sk-new", ProviderWriteFault::BeforeAuthReplace).unwrap();
    assert_eq!(report.outcome, ProviderWriteOutcome::Restored);
    assert!(report.rollback_verified);
    assert_eq!(fs::read_to_string(fixture.config()).unwrap(), "old-config");
    assert_eq!(read_auth_key(&fixture.auth()).unwrap().as_deref(), Some("old"));
}
```

Cover originally missing files, backup write failure, each replace boundary,
verification mismatch, rollback failure classification, target-file symlinks,
concurrent lock rejection, exact template bytes, and manifest hashes.

- [ ] **Step 2: Run and verify failures**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml codex_provider::tests -- --nocapture
```

Expected: transaction tests fail because the writer, report, manifest, and fault boundaries do not exist.

- [ ] **Step 3: Implement templates, backup manifest, commit, and rollback**

Expose pure/testable boundaries:

```rust
pub struct ProviderPaths {
    pub codex_home: PathBuf,
    pub backup_root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderWriteOutcome {
    Committed,
    FailedBeforeMutation,
    Restored,
    RecoveryRequired,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderWriteReport {
    pub outcome: ProviderWriteOutcome,
    pub backup_dir: Option<String>,
    pub config_path: String,
    pub auth_path: String,
    pub codex_was_running: bool,
    pub write_verified: bool,
    pub rollback_verified: bool,
    pub error_code: Option<String>,
}

pub fn write_provider_files_at(
    paths: &ProviderPaths,
    api_key: &str,
    codex_was_running: bool,
) -> Result<ProviderWriteReport, ProviderWriteError>;
```

Render the exact approved templates. Reject symlink destination files. Allow a
canonicalized `.codex` directory but keep both exact destinations below it.
Create `orangeapi-backups/<unix-millis>-<uuid>/`, fsync backup files and manifest,
then use the existing atomic-write pattern for each destination. Verify expected
SHA-256 and parsed auth key. Roll back both preimages on every post-mutation
failure; delete a destination whose preimage was absent. Verify rollback before
reporting `restored`; otherwise report `recovery_required` with backup path.
Once process shutdown or backup creation has begun, return a structured report
for every terminal transaction outcome instead of rejecting the Tauri command;
this lets the renderer offer Restart Codex even after a verified rollback.

- [ ] **Step 4: Run transaction tests repeatedly**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml codex_provider::tests -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml codex_provider::tests -- --nocapture
```

Expected: both runs pass, proving fixtures do not leak state and concurrent tests remain deterministic.

- [ ] **Step 5: Commit the transaction**

```powershell
git add src-tauri/src/app/codex_provider.rs src-tauri/src/app/atomic_file.rs
git commit -m "feat: install Codex provider transactionally"
```

## Task 7: Process-Safe Write and Restart Commands

**Files:**
- Modify: `src-tauri/src/app/codex_theme.rs`
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write failing process-order tests around a fake runtime**

Extract orchestration behind a small sync runtime boundary and assert ordering:

```rust
#[test]
fn running_codex_quits_and_settles_before_any_file_mutation() {
    let runtime = FakeRuntime::running();
    let writer = RecordingWriter::default();
    orchestrate_provider_write(&runtime, &writer, "sk-new").unwrap();
    assert_eq!(runtime.events(), ["detect", "quit", "settle", "write"]);
}

#[test]
fn restart_quits_running_instance_then_launches_but_stopped_instance_only_launches() {
    assert_eq!(restart_events(FakeRuntime::running()), ["detect", "quit", "settle", "launch"]);
    assert_eq!(restart_events(FakeRuntime::stopped()), ["detect", "launch"]);
}
```

- [ ] **Step 2: Run and verify failures**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml process_safe_provider -- --nocapture
```

Expected: FAIL because process orchestration is not exposed.

- [ ] **Step 3: Expose and wire existing Codex runtime primitives**

In `codex_theme.rs`, expose backend-only functions that reuse
`installed_codex_path`, `codex_running`, `quit_codex`, `CONFIG_SETTLE`, and
`launch_codex_plain`:

```rust
pub fn stop_for_external_config_write() -> Result<bool, AppError> {
    let installed = installed_codex_path()?;
    let was_running = codex_running();
    if was_running {
        quit_codex(&installed)?;
        std::thread::sleep(CONFIG_SETTLE);
    }
    Ok(was_running)
}

pub fn restart_codex_plain() -> Result<(), AppError> {
    let installed = installed_codex_path()?;
    if codex_running() {
        quit_codex(&installed)?;
        std::thread::sleep(CONFIG_SETTLE);
    }
    launch_codex_plain()
}
```

The write command must resolve the cached key, enter the provider write lock,
run `stop_for_external_config_write` inside `spawn_blocking`, perform the
transaction, re-project enabled state only for `committed`, and return the report
for committed/restored/recovery-required outcomes. Preflight failures may reject
with `CommandError`; post-shutdown outcomes must retain `codexWasRunning`. The
restart command runs `restart_codex_plain` in `spawn_blocking`. Register session,
login, keys, logout, CCS import, local write, and restart in `lib.rs`. Every auth
command derives `OrangeProxy` from the current persisted system/direct/custom
proxy setting, rebuilding the long-lived client only when that fingerprint
changes.

- [ ] **Step 4: Run process and command tests**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml process_safe_provider -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml commands:: -- --nocapture
```

Expected: ordering, unsupported-platform, error-code, and command registration tests pass.

- [ ] **Step 5: Commit process-safe commands**

```powershell
git add src-tauri/src/app/codex_theme.rs src-tauri/src/commands.rs src-tauri/src/lib.rs
git commit -m "feat: coordinate Codex provider writes and restart"
```

## Task 8: TypeScript IPC Contracts

**Files:**
- Modify: `src/shared/types.ts`
- Modify: `src/services/managerApi.ts`
- Modify: `src/services/managerApi.test.ts`

- [ ] **Step 1: Write failing invocation and guard tests**

```typescript
it("invokes API configuration commands without sending secrets back from key actions", async () => {
  window.__TAURI_INTERNALS__ = {};
  invokeMock.mockResolvedValueOnce(SESSION).mockResolvedValueOnce(KEYS);

  await managerApi.apiConfigSession();
  await managerApi.apiConfigKeys();
  await managerApi.apiConfigImportCcs(41);

  expect(invokeMock).toHaveBeenNthCalledWith(1, "api_config_session");
  expect(invokeMock).toHaveBeenNthCalledWith(2, "api_config_keys");
  expect(invokeMock).toHaveBeenNthCalledWith(3, "api_config_import_ccs", { keyId: 41 });
  expect(JSON.stringify(invokeMock.mock.calls)).not.toContain("sk-secret");
});

it("rejects malformed key statuses at the IPC boundary", async () => {
  window.__TAURI_INTERNALS__ = {};
  invokeMock.mockResolvedValue({ ...KEYS, items: [{ ...KEYS.items[0], status: "mystery" }] });
  await expect(managerApi.apiConfigKeys()).rejects.toMatchObject({ code: "contract_error" });
});
```

- [ ] **Step 2: Run and verify failures**

```powershell
npm test -- src/services/managerApi.test.ts
```

Expected: FAIL because API Configuration types and methods do not exist.

- [ ] **Step 3: Add frontend-safe types and guarded API methods**

Define exact contracts:

```typescript
export type ApiConfigConnection = "signed_out" | "connected" | "interrupted";
export type ApiConfigKeyStatus = "active" | "inactive" | "quota_exhausted" | "expired";

export interface ApiConfigSession {
  authenticated: boolean;
  email: string | null;
  remembered: boolean;
  connection: ApiConfigConnection;
  warning: string | null;
}

export interface ApiConfigKey {
  id: number;
  name: string;
  groupName: string;
  maskedKey: string;
  status: ApiConfigKeyStatus;
  quota: number;
  quotaUsed: number;
  expiresAt: string | null;
  actionable: boolean;
  enabled: boolean;
}

export interface ApiConfigKeyList {
  items: ApiConfigKey[];
  stale: boolean;
  fetchedAtUnix: number;
}

export type ApiConfigWriteOutcome =
  | "committed"
  | "failed_before_mutation"
  | "restored"
  | "recovery_required";

export interface ApiConfigWriteReport {
  outcome: ApiConfigWriteOutcome;
  backupDir: string | null;
  configPath: string;
  authPath: string;
  codexWasRunning: boolean;
  writeVerified: boolean;
  rollbackVerified: boolean;
  errorCode: string | null;
}
```

Add `apiConfigSession`, `apiConfigLogin`, `apiConfigKeys`, `apiConfigLogout`,
`apiConfigImportCcs`, `apiConfigWriteLocal`, and `apiConfigRestartCodex`. Validate
every enum, number, boolean, array, and nullable string like existing manager
guards. Browser fallback returns signed-out state and rejects mutating actions
with `desktop_required`; it never calls OrangeAPI from the WebView.

- [ ] **Step 4: Run service tests and typecheck**

```powershell
npm test -- src/services/managerApi.test.ts
npm run check
```

Expected: invocation, runtime guard, browser fallback, and no-secret tests pass.

- [ ] **Step 5: Commit the IPC layer**

```powershell
git add src/shared/types.ts src/services/managerApi.ts src/services/managerApi.test.ts
git commit -m "feat: expose API configuration IPC"
```

## Task 9: Navigation, Settings Entry, Icons, and Localization

**Files:**
- Modify: `src/app/App.tsx`
- Modify: `src/app/Rail.tsx`
- Modify: `src/app/views/Settings.tsx`
- Modify: `src/app/views/Settings.test.tsx`
- Modify: `src/app/icons.tsx`
- Modify: `src/app/i18n.tsx`
- Modify: `src/app/i18n.test.tsx`
- Create: `src/app/App.test.tsx`
- Create: `src/app/Rail.test.tsx`

- [ ] **Step 1: Write failing navigation and locale-completeness tests**

```typescript
it("places API configuration immediately below Home", () => {
  renderExpandedRail();
  expect(screen.getAllByRole("button").map((button) => button.textContent)).toEqual(
    expect.arrayContaining(["主页", "API 配置", "皮肤", "设置"]),
  );
  expect(screen.getByText("API 配置").compareDocumentPosition(screen.getByText("主页")))
    .toBe(Node.DOCUMENT_POSITION_PRECEDING);
});

it("keeps every API configuration key present in all locales", () => {
  for (const lang of LANGS) {
    expect(missingKeys(lang.code)).not.toContainEqual(expect.stringMatching(/^config\./));
  }
});
```

Add a Settings test that the former disabled row calls `onOpenConfig` and no
longer shows Coming soon.

- [ ] **Step 2: Run and verify failures**

```powershell
npm test -- src/app/App.test.tsx src/app/Rail.test.tsx src/app/views/Settings.test.tsx src/app/i18n.test.tsx
```

Expected: FAIL because config is not a rail section and the settings entry is disabled.

- [ ] **Step 3: Implement navigation, icons, and all locale strings**

Change `RailSection` to `"home" | "config" | "themes" | "settings"`; order the
items Home, API Configuration, Themes, Settings; and map `view === "config"` to
the config rail section. Keep `CodexConfig.onBack` returning to Settings for
compact entry. Enable the Settings row with `onOpenConfig` and a chevron.

Add `key`, `eye`, `eyeOff`, `logOut`, and `link` to the existing local stroke
icon map. Do not add a second icon package.

Replace the placeholder copy with the complete `config.*` key family for ZH,
EN, FR, ZH_TW, DE, KO, JA, RU, AR, ES, and PT_BR. The key family must cover:
title/service/signed-out/connected/interrupted, email/password/show/hide,
remember/login/logging-in/logout, refresh, empty/stale/retry, all four key
statuses, group/quota/expiry/unlimited/enabled, import/write, confirmation,
Codex-close warning, backup path, sent-to-CCS, write success, restart/restarting,
and each stable backend error. Preserve locale object completeness typing.

- [ ] **Step 4: Run navigation/i18n tests and typecheck**

```powershell
npm test -- src/app/App.test.tsx src/app/Rail.test.tsx src/app/views/Settings.test.tsx src/app/i18n.test.tsx
npm run check
```

Expected: route order, compact entry, focus, and all 11 locale dictionaries pass.

- [ ] **Step 5: Commit navigation and copy**

```powershell
git add src/app/App.tsx src/app/Rail.tsx src/app/views/Settings.tsx src/app/views/Settings.test.tsx src/app/icons.tsx src/app/i18n.tsx src/app/i18n.test.tsx src/app/App.test.tsx src/app/Rail.test.tsx
git commit -m "feat: add API configuration navigation"
```

## Task 10: Signed-Out Login Experience

**Files:**
- Create: `src/app/views/apiConfig/ApiLoginForm.tsx`
- Create: `src/app/views/apiConfig/ApiLoginForm.test.tsx`
- Modify: `src/app/views/CodexConfig.tsx`
- Create/Modify: `src/app/views/CodexConfig.test.tsx`

- [ ] **Step 1: Write failing login-view tests**

```typescript
it("prefills email, never prefills password, and submits remember preference", async () => {
  const user = userEvent.setup();
  const login = vi.fn().mockResolvedValue(CONNECTED_SESSION);
  renderLogin({ email: "saved@example.com", login });
  expect(screen.getByLabelText("邮箱")).toHaveValue("saved@example.com");
  expect(screen.getByLabelText("密码")).toHaveValue("");
  await user.type(screen.getByLabelText("密码"), "pw");
  await user.click(screen.getByRole("checkbox", { name: "记住登录" }));
  await user.click(screen.getByRole("button", { name: "登录" }));
  expect(login).toHaveBeenCalledWith("saved@example.com", "pw", true);
});

it("shows unsupported 2FA separately from invalid credentials", async () => {
  renderLogin({ loginError: { code: "orange_2fa_unsupported", message: "" } });
  expect(screen.getByRole("alert")).toHaveTextContent("暂不支持两步验证");
});
```

Also test Enter submit, password eye button, duplicate-submit disabling,
remember-save warning, silent-restore loading, and desktop-required browser copy.

- [ ] **Step 2: Run and verify failures**

```powershell
npm test -- src/app/views/apiConfig/ApiLoginForm.test.tsx src/app/views/CodexConfig.test.tsx
```

Expected: FAIL because the placeholder has no login workflow.

- [ ] **Step 3: Implement signed-out orchestration**

`CodexConfig` loads `managerApi.apiConfigSession()` once, shows a stable loading
hero, and renders `ApiLoginForm` when signed out. Keep password only in local
component state and clear it after every failed or successful attempt. Use
`type="email"`, `type="password"`, `autoComplete="username"` and
`autoComplete="current-password"`; the eye icon button must have a localized
accessible label. Render backend errors through stable error-code copy, never
through raw secret-bearing payloads. On login success, immediately call the key
refresh and transition to the signed-in view.

- [ ] **Step 4: Run login tests, accessibility lint, and typecheck**

```powershell
npm test -- src/app/views/apiConfig/ApiLoginForm.test.tsx src/app/views/CodexConfig.test.tsx
npm run lint
npm run check
```

Expected: all signed-out, keyboard, busy, and error tests pass with no a11y lint warnings.

- [ ] **Step 5: Commit the login view**

```powershell
git add src/app/views/apiConfig/ApiLoginForm.tsx src/app/views/apiConfig/ApiLoginForm.test.tsx src/app/views/CodexConfig.tsx src/app/views/CodexConfig.test.tsx
git commit -m "feat: add OrangeAPI login view"
```

## Task 11: Signed-In Key List and Actions

**Files:**
- Create: `src/app/views/apiConfig/ApiKeyList.tsx`
- Create: `src/app/views/apiConfig/ApiKeyList.test.tsx`
- Modify: `src/app/views/CodexConfig.tsx`
- Modify: `src/app/views/CodexConfig.test.tsx`

- [ ] **Step 1: Write failing list/action tests**

```typescript
it("marks the local key enabled and disables unusable actions", async () => {
  renderKeyList({
    items: [activeKey({ id: 1, enabled: true }), inactiveKey({ id: 2 })],
  });
  expect(screen.getByText("已启用")).toBeInTheDocument();
  const inactive = screen.getByTestId("api-key-2");
  expect(within(inactive).getByRole("button", { name: "导入 CCS" })).toBeDisabled();
  expect(within(inactive).getByRole("button", { name: "写入本机" })).toBeDisabled();
});

it("confirms replacement, writes by ID, then exposes restart and backup path", async () => {
  const user = userEvent.setup();
  api.apiConfigWriteLocal.mockResolvedValue(WRITE_REPORT);
  renderConnected();
  await user.click(screen.getByRole("button", { name: "写入本机" }));
  expect(screen.getByRole("dialog")).toHaveTextContent("config.toml");
  expect(screen.getByRole("dialog")).toHaveTextContent("Codex");
  await user.click(screen.getByRole("button", { name: "备份并覆盖" }));
  expect(api.apiConfigWriteLocal).toHaveBeenCalledWith(1);
  expect(await screen.findByRole("button", { name: "重启 Codex" })).toBeVisible();
  expect(screen.getByText(WRITE_REPORT.backupDir)).toBeVisible();
});
```

Also test 20-row pages, refresh, stale retained list, empty state, CCS sent
message, missing CCS, independent busy states, logout retaining email in the
returned signed-out view, write-restored error, recovery-required backup path,
restart retry, and fixed lower-right logout without obscuring page content.

- [ ] **Step 2: Run and verify failures**

```powershell
npm test -- src/app/views/apiConfig/ApiKeyList.test.tsx src/app/views/CodexConfig.test.tsx
```

Expected: FAIL because the signed-in list and action sheet do not exist.

- [ ] **Step 3: Implement the signed-in state machine**

Render a compact connection band, account email, host, and refresh icon. Keep
20-row client pages with stable dimensions. Each row shows name, group, masked
key, status, quota/usage, expiry, Enabled tag, and two fixed-size action buttons.
Use a centered-in-expanded `Sheet` for backup-and-replace confirmation; the body
must state that running Codex will close before either file changes. On success,
update the matching row locally, show backup path and Restart Codex. Refresh
from the backend after write without hiding the success report. Logout clears
the list and returns to the form with the email from the signed-out session.

- [ ] **Step 4: Run all API Configuration frontend tests**

```powershell
npm test -- src/app/views/apiConfig/ApiKeyList.test.tsx src/app/views/apiConfig/ApiLoginForm.test.tsx src/app/views/CodexConfig.test.tsx
npm run check
```

Expected: login, list, pagination, action, write, restart, stale, and logout tests pass.

- [ ] **Step 5: Commit the signed-in view**

```powershell
git add src/app/views/apiConfig/ApiKeyList.tsx src/app/views/apiConfig/ApiKeyList.test.tsx src/app/views/CodexConfig.tsx src/app/views/CodexConfig.test.tsx
git commit -m "feat: manage OrangeAPI keys in Codex"
```

## Task 12: Responsive Styling and Visual Verification

**Files:**
- Modify: `src/app/styles.css`
- Modify: frontend tests only if visual inspection reveals a regression

- [ ] **Step 1: Add layout-contract tests before CSS**

Assert the structural hooks and accessibility rules that prevent unstable layout:

```typescript
it("keeps key actions in a stable toolbar and logout outside list rows", () => {
  const { container } = renderConnected();
  expect(container.querySelectorAll(".api-key-actions")).toHaveLength(KEYS.items.length);
  expect(container.querySelector(".api-config-logout")).not.toBeNull();
  expect(container.querySelector(".api-key-list .api-config-logout")).toBeNull();
});
```

- [ ] **Step 2: Run the structural tests**

```powershell
npm test -- src/app/views/apiConfig/ApiKeyList.test.tsx src/app/views/CodexConfig.test.tsx
```

Expected: FAIL until the final class structure is present.

- [ ] **Step 3: Add scoped styles**

Add `.api-config-*` rules only. Use existing CSS variables; no new one-hue theme,
gradients, decorative cards, or nested cards. Inputs are full-width with stable
44px height; action buttons have fixed icon/text tracks; key rows use responsive
grid tracks; long emails, group names, and masked keys wrap or ellipsize without
overlap. Reserve lower padding for the logout row rather than overlaying it.
At compact width, actions wrap below metadata; expanded width keeps two columns.
Add `[dir="rtl"]` alignment and preserve reduced-motion behavior.

- [ ] **Step 4: Run frontend gates and inspect desktop/narrow screenshots**

```powershell
npm run check
npm run lint
npm test
npm run build
```

Start or reuse Vite and inspect at least 400x640, 760x720, and 1200x800. Capture
signed-out, signed-in, stale, empty, confirmation, and success states. Verify no
horizontal overflow, text clipping, incoherent overlap, focus loss, blank view,
or content hidden by logout. Use mocked IPC states for non-destructive browser
inspection; do not place a real key in screenshots.

Expected: all frontend gates pass and every inspected state is readable in light/dark and LTR/RTL.

- [ ] **Step 5: Commit styling**

```powershell
git add src/app/styles.css src/app/views/apiConfig/ApiKeyList.test.tsx src/app/views/CodexConfig.test.tsx
git commit -m "style: polish API configuration view"
```

## Task 13: Full Verification, Review, and Delivery

**Files:**
- Modify: only files required by failures or review findings

- [ ] **Step 1: Run the complete frontend gate**

```powershell
npm run check
npm run lint
npm run test
npm run build
```

Expected: all commands exit 0 with no TypeScript, ESLint, Vitest, build, or renderer-entry failures.

- [ ] **Step 2: Run the complete Rust gate**

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --workspace
cargo test --manifest-path crates/codex-mac-engine/Cargo.toml --all-targets
cargo test --manifest-path crates/codex-win-engine/Cargo.toml --all-targets
cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --all-targets
```

Expected: formatting, clippy, app tests, and all three standalone engine suites exit 0.

- [ ] **Step 3: Run native and non-destructive integration checks**

On Windows, launch the Tauri client when local policy permits. Verify public
settings reachability, signed-out startup, credential-store creation/deletion
with a test account, OpenAI-only list projection, CCS missing-handler behavior,
and process-safe write against an isolated smoke Codex home. Never overwrite the
real `~/.codex` during automated checks. Record any Smart App Control limitation
as a test gap rather than claiming native success.

macOS-specific keychain/process behavior must be proven by required macOS CI.

- [ ] **Step 4: Review and iterate to zero findings**

```powershell
codex review --base main
```

Address every actionable finding, rerun the affected focused tests, then rerun
the complete frontend and Rust gates. Repeat `codex review --base main` until
it reports no actionable findings.

- [ ] **Step 5: Perform requirement-by-requirement completion audit**

Check evidence for: fixed endpoint; OrangeAPI repo untouched; email/password
login; remembered refresh token only; logout retaining email; OpenAI-only full
status list; enabled local match; CCS import; Windows/macOS backup-before-replace;
graceful quit/settle; verified rollback; success restart; no secret in renderer
or logs; top-level rail order; compact entry; all locales; and all test gates.
Treat missing native/macOS evidence as incomplete until CI or an explicit test
gap is reported.

- [ ] **Step 6: Commit final fixes and open the required PR**

```powershell
git status --short
git add -u
git commit -m "fix: address API configuration review findings"
git push -u origin codex/orangeapi-api-config
gh pr create --title "feat: add OrangeAPI API configuration" --body "## Summary`n- add secure OrangeAPI login and remembered sessions`n- list OpenAI keys with CC Switch and local Codex actions`n- back up, replace, verify, and restart Codex safely`n`n## Test plan`n- npm run check && npm run lint && npm run test && npm run build`n- cargo clippy/test gates for app and all engines`n- desktop and narrow visual verification"
```

If review produced no final tracked changes, skip the final-fix commit after
confirming `git diff --exit-code`; do not create an empty commit. Do not include
the user's pre-existing untracked `AGENTS.md`. Wait for Frontend,
Rust macOS, and Rust Windows required checks. Merge with squash only after all
required checks pass, following the repository's protected-main policy.
