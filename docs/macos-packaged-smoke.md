# macOS native shell and packaged smoke

## Language policy

- The product name is always **Codex App Manager** in the bundle metadata,
  title, app-menu heading, About metadata, and Quit label.
- Native command labels follow the language selected inside the app. The
  frontend sends that language only after both quit-event listeners are live;
  changing the app language rebuilds the native menu without restarting.
- The custom Quit item keeps `Cmd+Q`. Edit actions remain Tauri/macOS
  predefined items with their standard selectors and shortcuts. The frameless
  window uses explicit `Cmd+M`/`Cmd+W` menu items that call Tauri's minimize and
  close APIs, because AppKit's default Window responders do not target this
  undecorated WebView window reliably.
- Unsupported or malformed language tags fall back to English. The Rust table
  covers the same 11 locale codes as the WebView catalogue.

## Startup and activation contract

The main window is packaged as initially hidden. `QuitConfirm` registers
`app://confirm-quit` and `app://quit-blocked`, then waits for the backend token
injected into that document. Every `PageLoad Started` creates a new random token
and generation; `PageLoad Finished` delivers the current token to the loaded
document. `frontend_ready` must return the exact generation and token, so an IPC
call left over from an older document cannot show the window or drain events
into replacement listeners that are not live yet. After an accepted handshake,
the backend localizes the menu, fixes the title, shows/focuses the window, and
drains startup close/quit events in order. Repeated events are coalesced by kind
while the frontend is not ready.

If readiness takes more than 10 seconds, the backend logs an error, reveals the
window, and enters a degraded native-dialog mode without discarding queued
events. Pending and later Close/Cmd+Q decisions are shown serially with native
dialogs instead of being sent to missing WebView listeners. A native Quit
confirmation re-reads the current operation phase immediately before exiting;
committing/finishing remains blocked even if the phase changed while the dialog
was open. Degraded delivery stays latched for that generation even if its
handshake arrives late; only a subsequent `PageLoad Started` can begin a fresh
frontend-ready generation. The frontend keeps retrying a failed listener
registration or readiness IPC call with bounded exponential backoff and always
reads the latest document token. A partially registered listener set is removed
before retrying. Failed unminimize/show/focus and event delivery attempts are
logged; the app requests informational Dock attention as a fallback. A ready
second launch always attempts unminimize, show, and focus independently so one
failed step does not suppress the others; before readiness, activation is queued
without showing the hidden window.

## Repeatable PR matrix

The `macOS packaged smoke` workflow builds an isolated bundle identifier
(`io.github.wangnov.codexappmanager.smoke`) and never reads release secrets.
The harness also creates a private `0700` child of the system temporary
directory. Rust accepts that data-directory override only when its exact leaf
matches the sanitized smoke run ID; a missing, partial, symlinked, broad-access,
or non-temporary override fails closed instead of falling back to real manager
settings, provenance, operation locks, staging, or recovery state.

| Runner | Target | Per-app Apple language | Expected app language | Runtime coverage |
| --- | --- | --- | --- | --- |
| `macos-15` | `aarch64-apple-darwin` | `zh-Hans` | `zh-CN` | arm64, CJK menu |
| `macos-15-intel` | `x86_64-apple-darwin` | `en` | `en` | Intel x64, Latin menu |

Each row checks:

1. isolated bundle ID, product metadata, executable, and Mach-O architecture;
2. per-document readiness-token injection plus frontend-ready IPC (which also
   proves the packaged WebView loaded under the configured CSP) and the expected
   localized native menu;
3. one live process after a second launch, including restoration from both
   minimized and hidden states;
4. `Cmd+W` close interception and visible confirmation-event delivery;
5. both the `Cmd+Q` accelerator and clicking the native Quit item route through
   `cam-quit` instead of bypassing the phase-aware confirmation guard.

The test accepts unsigned/ad-hoc bundles. Developer ID signing, updater signing,
notarization, and stapling remain exclusively in the tag-driven release job.

## Local command

```sh
npm run tauri build -- \
  --config src-tauri/tauri.smoke.conf.json \
  --bundles app \
  --target aarch64-apple-darwin \
  --no-sign

bash scripts/macos-packaged-smoke.sh \
  "target/aarch64-apple-darwin/release/bundle/macos/Codex App Manager.app" \
  --expected-arch arm64 \
  --expected-lang en \
  --apple-language en
```

The local smoke needs macOS Accessibility permission for the terminal host
because it verifies the real menu and sends `Cmd+M`, `Cmd+H`, `Cmd+W`, and
`Cmd+Q` through System Events.
