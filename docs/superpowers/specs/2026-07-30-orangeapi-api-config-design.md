# OrangeAPI API Configuration Integration Design

Date: 2026-07-30
Status: Approved in conversation

## 1. Objective

Add a first-class **API Configuration** view to Codex App Manager. The view
authenticates against the existing OrangeAPI/Sub2API deployment, lists the
signed-in user's OpenAI API keys, imports a selected key into CC Switch, and
writes a selected key into the local Codex configuration on Windows and macOS.

This work changes only `E:\ForkProject\Codex-App-Manager`.
`E:\ForkProject\orangeapi` is a read-only API and behavior reference. The
existing Codex installation/update implementation remains unchanged.

## 2. Approved Scope

- Add **API Configuration** as a top-level expanded-rail item immediately below
  Home. Keep the existing Settings entry as a compact-mode route to the view.
- Use the fixed OrangeAPI deployment `https://token.cylonai.cn`, stored as one
  source-level constant so a fork can change it in one place.
- Support email/password login only in v1.
- Do not implement registration, OAuth, Turnstile completion, or TOTP entry.
- Detect Turnstile/TOTP requirements and show an explicit unsupported message.
- Offer a **Remember login** checkbox. Persist a refresh token only when it is
  checked; never persist the password.
- On logout, revoke and remove tokens but retain the last email address.
- Fetch and display only keys whose `group.platform` is `openai`.
- Display all OpenAI key states, but enable actions only when the key and group
  are active, the key is unexpired, and its configured quota is not exhausted.
- Detect which server key matches the current local `OPENAI_API_KEY` and mark it
  **Enabled**.
- Support CC Switch import and backup-then-replace local Codex configuration.
- After a verified local write, expose a **Restart Codex** action.
- Implement and test the behavior on Windows and macOS.

## 3. External API Contract

All endpoints use the `/api/v1` prefix and the standard OrangeAPI response
envelope. Authenticated calls send `Authorization: Bearer <access_token>`.

### 3.1 Public settings

`GET /api/v1/settings/public`

Read at login time to detect `turnstile_enabled`, `turnstile_site_key`,
`totp_enabled`, `site_name`, and `api_base_url`. The fixed deployment currently
reports Turnstile and TOTP disabled. If that changes, v1 returns a targeted
unsupported-auth error rather than an invalid-password error.

### 3.2 Login

`POST /api/v1/auth/login`

```json
{
  "email": "user@example.com",
  "password": "secret"
}
```

The normal response contains `access_token`, `refresh_token`, `expires_in`,
`token_type`, and `user`. A response containing `requires_2fa: true` and a
`temp_token` is rejected with an explicit v1 limitation message. Login is
rate-limited by the server to 20 attempts per minute; HTTP 429 is surfaced as a
clear rate-limited state.

### 3.3 Refresh and logout

`POST /api/v1/auth/refresh`

```json
{ "refresh_token": "..." }
```

Refresh-token rotation is mandatory: a newly returned refresh token replaces
the old credential before the command reports success.

`POST /api/v1/auth/logout`

```json
{ "refresh_token": "..." }
```

Logout is best effort remotely and unconditional locally. Network failure must
not prevent local token deletion.

### 3.4 API key list

`GET /api/v1/keys?page=N&page_size=50&sort_by=created_at&sort_order=desc`

The response is paginated with `items`, `total`, `page`, `page_size`, and
`pages`. The desktop client follows all pages and filters items client-side to
`group.platform === "openai"` because the endpoint has no platform filter.

Relevant fields are `id`, `key`, `name`, `group_id`, `group`, `status`, `quota`,
`quota_used`, `expires_at`, rate limits, usage windows, and timestamps. Valid
statuses are `active`, `inactive`, `quota_exhausted`, and `expired`.

## 4. Architecture

### 4.1 Rust ownership

Authentication, secrets, HTTP calls, the full API-key cache, CC Switch deep-link
construction, local file mutations, and Codex restart behavior live in Rust.
React receives only presentation-safe data and invokes commands by API-key ID.

Planned focused modules:

- `orange_api.rs`: fixed endpoint, HTTP client construction, API envelope
  parsing, login/refresh/logout, public settings, and paginated key retrieval.
- `orange_session.rs`: in-memory access-token state, expiry, refresh rotation,
  remembered credential access, last-email metadata, and key cache.
- `codex_provider.rs`: local-key inspection, CC Switch import, configuration
  rendering, backup/replace transaction, verification, and restart dispatch.

The modules expose typed operations through Tauri commands and
`src/services/managerApi.ts`. They do not add OrangeAPI behavior to the existing
installer/update engines.

### 4.2 HTTP client and proxy behavior

One long-lived `reqwest::Client` is used for login, refresh, logout, and key
retrieval. It has a stable App Manager User-Agent and follows the current App
Manager system/direct/custom proxy setting. Keeping these calls in one client
avoids splitting OrangeAPI's optional IP/User-Agent session binding across the
WebView and Rust.

Before an authenticated request, the session refreshes when the access token is
near expiry. A 401 triggers at most one refresh and one retry. A second 401 ends
the authenticated session. There is no recursive retry.

### 4.3 Secret storage

- Access tokens and complete API keys exist only in Rust process memory.
- Remembered refresh tokens use Windows Credential Manager or macOS Keychain
  behind a small credential-store trait. Tests use an in-memory implementation.
- The last email and non-secret remember preference use an atomic JSON metadata
  file in the App Manager data directory.
- Passwords are accepted only as command arguments, never persisted, returned,
  or logged.
- If saving a remembered credential fails, login remains valid for the current
  process but the UI warns that login was not remembered.

### 4.4 Frontend-safe key model

The frontend key shape contains:

- ID, name, group name, status, quota/usage, expiry, and timestamps.
- A Rust-generated masked value, never the full API key.
- `actionable`, calculated from current status/quota/expiry.
- `enabled`, calculated by matching the local `auth.json` key.

Import and write commands accept only the ID. Rust resolves the full value from
the latest authenticated cache. A missing/stale ID returns a refresh-required
error.

## 5. User Interface

### 5.1 Navigation

Expanded rail order:

1. Home
2. API Configuration
3. Themes
4. Settings

The existing `CodexConfig` placeholder becomes the real API Configuration view.
Settings retains a link to the view for compact mode, where the rail is hidden.
The implementation follows existing colors, typography, spacing, focus,
animation, button, banner, list, and sheet patterns.

### 5.2 Signed-out state

- Header: API Configuration, fixed service identity, and Signed out status.
- Form: email, password, password-visibility icon, Remember login checkbox, and
  primary Login button.
- The last email is prefilled. The password is always empty on a new process.
- Startup first attempts silent refresh when a remembered token exists.
- Loading disables duplicate submit. Enter submits the form.
- Invalid credentials, disabled account, rate limiting, unsupported 2FA,
  unsupported Turnstile, and network failure have distinct copy.

### 5.3 Signed-in state

- Header: Connected state, account email, fixed service host, and refresh icon.
- List: OpenAI keys only, newest first, with deterministic 20-row client-side
  pages and previous/next controls after Rust has loaded all server pages.
- Row: key name, group, masked key, status, quota, expiry, and Enabled badge.
- Rows expose **Import to CCS** and **Write locally** only when key status and
  group status are active, `expires_at` is absent or in the future, and `quota`
  is unlimited (`0`) or `quota_used < quota`.
- Inactive, quota-exhausted, and expired rows remain visible with disabled
  actions and an explanatory tooltip.
- An empty state is shown when no OpenAI keys exist.
- A list request failure preserves the session and last successful list, marks
  it stale, and offers retry.
- Logout is anchored at the lower right and remains reachable without covering
  list content.

### 5.4 Write interaction

Write locally opens a confirmation sheet naming `config.toml` and `auth.json`
and stating that both existing files will be backed up and replaced. The sheet
also states that a running Codex must close before the write. On verified
success, the selected row becomes Enabled, a success result shows the backup
location, and a Restart Codex button appears. The button reopens Codex after a
write that closed it; if Codex was already stopped, the button starts it.

## 6. CC Switch Import

Rust constructs this deep link from the cached key and fixed configuration:

- `resource=provider`
- `app=codex`
- `model=gpt-5.5`
- `name=酸橘子` (fall back to `OrangeAPI` if public settings have no name)
- `homepage=https://token.cylonai.cn`
- `endpoint=https://token.cylonai.cn`
- `apiKey=<cached full key>`
- `configFormat=json`
- `usageEnabled=true`
- `usageScript=<standard-base64 OrangeAPI usage script>`
- `usageAutoInterval=30`

The usage script queries `{{baseUrl}}/v1/usage` with
`Authorization: Bearer {{apiKey}}`, matching OrangeAPI's current frontend.

The generic public `open_url` command remains HTTP(S)-only. A dedicated command
builds and opens the hard-coded `ccswitch` scheme internally, so the renderer
cannot request arbitrary custom protocols. Success means the OS accepted the
protocol launch; the UI says **Sent to CCS**, not **Imported**, because CCS has
no completion callback. Missing protocol registration is reported explicitly.

## 7. Local Codex Configuration

The generated `~/.codex/config.toml` exactly follows OrangeAPI's current OpenAI
Codex template:

```toml
model_provider = "OpenAI"
model = "gpt-5.5"
review_model = "gpt-5.5"
model_reasoning_effort = "xhigh"
disable_response_storage = true
network_access = "enabled"
windows_wsl_setup_acknowledged = true

[model_providers.OpenAI]
name = "OpenAI"
base_url = "https://token.cylonai.cn/v1"
wire_api = "responses"
requires_openai_auth = true

[features]
goals = true
```

The generated `~/.codex/auth.json` is:

```json
{
  "OPENAI_API_KEY": "<selected full key>"
}
```

This is an intentional full replacement, not a merge.

### 7.1 Two-file transaction

1. Acquire a process-wide exclusive provider-write lock.
2. Resolve the detected Codex installation and determine whether it is running.
3. If running, request a graceful quit, wait for process exit, and then wait the
   existing two-second configuration settle interval. Never force-kill. This is
   required because Codex persists in-memory configuration after process exit
   and would otherwise overwrite the replacement.
4. Resolve the real Codex home and reject unsafe symlink/path cases.
5. Read both preimages and record whether either file was absent.
6. Create an App Manager backup directory under
   `orangeapi-backups/<timestamp-id>/` containing original files and a manifest
   with source paths, existence markers, timestamp, and SHA-256 hashes.
7. Stage replacement files on the same filesystem as `~/.codex` and atomically
   replace each destination.
8. Re-read both files and verify exact expected bytes/hashes plus the parsed
   `OPENAI_API_KEY`.
9. On any failure, restore both preimages (including deleting a destination that
   was originally absent), verify the restoration, and retain transaction
   evidence.
10. Return one of: committed, failed without mutation, failed and restored, or
    recovery required. The last state includes the backup path for manual
    repair and whether Codex was closed.

Backups are retained and never automatically deleted by this feature.

### 7.2 Enabled detection

List/status refresh reads `auth.json` defensively and extracts only
`OPENAI_API_KEY`. Exact equality with a server key sets `enabled=true` on one
row. Missing, malformed, or unmatched local data yields no enabled row and does
not fail the server-key list.

### 7.3 Restart

Restart reuses existing platform detection and launch paths. If Codex is
running, request a graceful quit and wait for process exit before launching the
detected installation. Never force-kill. If Codex is stopped, launch directly.
A quit timeout or launch failure does not roll back a verified configuration;
the UI keeps Enabled state and offers retry.

## 8. Error and Logging Rules

- Map invalid credentials, disabled account, 429, offline/timeout, malformed
  server data, unsupported auth, credential-store failure, stale key cache,
  protocol-handler failure, transaction failure, and restart failure to
  distinct user-facing errors.
- List failures do not clear a valid session.
- Logout clears local secrets even when remote logout fails.
- Never log or serialize passwords, access tokens, refresh tokens, full API
  keys, Authorization headers, request bodies containing secrets, or generated
  deep links.
- Login, refresh, key-list, CCS import, local write, and restart have independent
  busy/error states and reject duplicate concurrent commands.

## 9. Testing Strategy

### 9.1 Rust tests

- HTTP contract: login body/path, response envelope, invalid credentials, 429,
  unsupported 2FA/Turnstile, expiry refresh, rotated refresh tokens, one-time
  401 retry, and terminal 401.
- Key loading: multi-page traversal, ordering, OpenAI filtering, all status
  mappings, masking, actionability, and local Enabled matching.
- Credential abstraction: remember, do-not-remember, startup recovery, rotation,
  save failure, and logout while retaining email.
- CC Switch: parse every query field; verify endpoint, model, percent encoding,
  API-key encoding, Base64 script, and special characters.
- File transaction in isolated temporary homes: no existing files, both files
  present, exact backups and hashes, exact replacement, successful verification,
  and injected failure/rollback at every mutation stage.
- Path safety: symlinks, malformed files, missing directories, and concurrent
  writes.
- Restart dispatch: running, stopped, graceful-quit timeout, and launch failure
  through fake platform adapters.

Tests never touch the user's real credential store or `~/.codex`.

### 9.2 Frontend tests

- Silent-login loading, signed-out form, email prefill, password visibility,
  remember checkbox, submit behavior, and auth error states.
- Signed-in header, refresh, empty/list/stale states, status labels, disabled
  actions, Enabled badge, write confirmation, verified-success/restart state,
  and logout.
- Manager API runtime guards reject malformed IPC data.
- Keyboard focus, accessible labels, disabled semantics, and no layout-obscuring
  fixed controls.

### 9.3 Completion verification

Run the repository's required local gates:

```text
npm run check
npm run lint
npm run test
npm run build
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --workspace
cargo test --manifest-path crates/codex-mac-engine/Cargo.toml --all-targets
cargo test --manifest-path crates/codex-win-engine/Cargo.toml --all-targets
cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --all-targets
codex review --uncommitted
```

Also inspect the running frontend at desktop and narrow window sizes for
overflow, overlap, loading, empty, error, list, confirmation, and success states.
Perform a Windows native smoke test locally when policy permits. macOS-specific
behavior must pass the required macOS CI because the current workstation cannot
execute a macOS binary.

## 10. Acceptance Criteria

- API Configuration is a top-level rail destination below Home and remains
  reachable in compact mode.
- A user can log in with email/password, optionally remember the session, and
  restore it after restart without persisting a password.
- Logout revokes/clears tokens and retains only the email.
- The page shows all and only OpenAI keys, accurate status/actionability, and
  the currently enabled local key.
- An actionable key can be sent to CC Switch using OrangeAPI-compatible fields.
- An actionable key can replace both Codex files on Windows and macOS only after
  a durable backup, with verified commit or verified rollback.
- A successful write exposes a working restart/start action for Codex.
- No secret appears in renderer state, logs, diagnostics, or user-facing errors.
- Focused tests cover the new API, session, list, secret-store, transaction,
  import, restart, and UI behavior, and all existing project quality gates pass.

## 11. Non-Goals

- Modifying or deploying OrangeAPI/Sub2API.
- Registration, password reset, OAuth, Turnstile completion, or TOTP login.
- Non-OpenAI keys or non-Codex local configuration formats.
- API-key creation, editing, deletion, or status changes.
- Changing Codex installation, update, uninstallation, or release behavior.
- Merging with existing Codex configuration; v1 intentionally backs up and
  replaces it.
