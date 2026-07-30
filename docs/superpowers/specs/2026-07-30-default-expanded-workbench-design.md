# Default Expanded Workbench Design

Date: 2026-07-30
Status: Approved in conversation

## 1. Objective

Fix the packaged application's missing-sidebar startup state by making the main
window open as the expanded workbench on every launch. The first visible frame
must be the `1100x720` workbench with the navigation rail present, including the
new API Configuration entry.

This change applies to Windows and macOS. It preserves the existing compact
`400x640` mode as an explicit per-session choice through **Collapse workspace**.

## 2. Root Cause

Three independent defaults currently describe startup as compact:

- `src-tauri/tauri.conf.json` creates a fixed, non-resizable `400x640` native
  window without a native shadow.
- `WindowModeProvider` initializes React state as `compact`.
- The initial document has no expanded-mode marker, so expanded CSS is absent
  until React applies one after mounting.

`Rail` intentionally renders only in expanded mode. Packaging therefore did
not remove the menu; the packaged application started in the mode that hides
it. Changing only one of these defaults would leave the native frame, React
tree, and first-paint CSS out of sync.

## 3. Approved Behavior

- Every new application process starts in expanded mode.
- The normal startup target is `1100x720` logical pixels.
- The navigation rail is present on the first rendered application frame.
- The initial native window is resizable, has a `960x640` logical-pixel minimum,
  is centered, and uses the native expanded-window shadow.
- On a work area smaller than the nominal expanded minimum or default, the
  existing expanded-size normalization keeps the complete window reachable.
- **Collapse workspace** remains available at the bottom of the rail. It
  switches to fixed `400x640` compact mode and hides the rail exactly as it does
  today.
- The compact top-bar expand control remains available after collapsing and
  restores expanded mode.
- A manually resized expanded size continues to be persisted in `localStorage`.
  Startup does not consume that value and uses the standard expanded target;
  after the user collapses, a later re-expansion continues to consume the
  remembered size exactly as it does today.
- Browser development and preview builds also start expanded so their first
  render matches packaged behavior.
- Window mode is not added to persistent user settings. Only the expanded size
  remains persistent. Each new process starts expanded even if the previous
  process was closed while compact.

## 4. Startup Contract

The native shell and renderer must agree before the window becomes visible:

1. Tauri's main-window configuration describes the expanded frame: `1100x720`,
   resizable, `960x640` minimum, centered, and shadowed.
2. During native window construction, the existing expanded sizing rules are
   applied before the frontend-ready handshake can show the window. This
   temporarily clears the declarative minimum, clamps the target to the current
   monitor work area, resizes the frame, and then establishes the effective
   expanded minimum. Clearing first is required when the monitor work area is
   smaller than the nominal `960x640` minimum.
3. The HTML document carries `data-window-mode="expanded"` from its initial
   parse, so expanded layout CSS is active before React's effects run.
4. `WindowModeProvider` initializes to `expanded`, matching both the document
   marker and native frame. Its mount effect keeps owning subsequent marker
   updates.

The application must not open a `400x640` native frame containing an expanded
React layout, nor briefly render the compact layout inside an expanded frame.

## 5. Component Responsibilities

### 5.1 Native configuration

`src-tauri/tauri.conf.json` is the declarative fallback and source for the
initial expanded window properties used by `WebviewWindowBuilder`. Existing
security, visibility, transparency, decorations, and centering behavior remain
unchanged except for the expanded size, resizability, minimum size, and shadow.

### 5.2 Native startup normalization

The main-window construction path reuses the window-mode module's expanded
constants and monitor-aware normalization after building the hidden window.
This keeps startup consistent with later mode transitions and prevents a
nominal `1100x720` frame from becoming unreachable on a smaller work area.
The expanded application path clears any stale or declarative minimum before
setting the normalized size, then installs the effective minimum afterward.
Failure to establish a valid initial frame is a startup error; the application
must not show a partially configured window.

### 5.3 Renderer first paint

`index.html` declares the expanded marker before styles load.
`WindowModeProvider` uses `expanded` as its initial state and updates existing
comments that currently describe compact-first behavior. No asynchronous IPC
round trip is needed to discover the startup mode because startup is fixed by
product policy.

### 5.4 Existing mode switching

The existing `set_window_mode` command remains the sole runtime transition
path. Compact mode still clears the expanded minimum before shrinking, disables
resizing, and disables the native shadow. Returning to expanded mode still
uses a valid remembered size when available, restores the minimum, enables
resizing and shadow, and keeps the frame inside the active monitor.

No changes are required to rail ordering, API Configuration routing, login,
key retrieval, local Codex writes, or CC Switch import behavior.

## 6. Error Handling

- Native startup sizing errors are logged without credentials or local Codex
  data and fail window construction through the existing startup error path.
- Runtime collapse or expansion errors retain the current mode and continue to
  use the existing warning behavior.
- Invalid remembered expanded sizes continue to fall back to `1100x720` and
  are clamped by the existing normalization rules.
- The frontend-ready visibility gate remains unchanged, so Windows does not
  expose an intermediate WebView2 frame while the renderer is loading.

## 7. Testing Strategy

Implementation follows test-driven development.

### 7.1 Frontend regression tests

- Change the window-mode startup test to require `expanded` on `<html>`, a
  visible navigation rail, and no expand control on first render.
- Start runtime transition tests from expanded, verify collapse produces
  compact mode and hides the rail, then verify re-expansion restores the rail
  and size memory behavior.
- Extend the frontend configuration contract coverage to read `index.html` and
  require its expanded marker, so the first-paint CSS cannot drift back to
  compact independently of React.

### 7.2 Native regression tests

- Extend `src/app/securityConfig.test.ts` to assert that the packaged Tauri
  main-window configuration contains the expanded default size, minimum size,
  resizability, and shadow.
- Define and test a single Rust startup-mode constant whose value is
  `WindowMode::Expanded`, and make native window construction pass that constant
  through `apply_window_mode` before showing the window. Retain the existing
  small-work-area, default-size, and placement cases as the sizing contract.
- Preserve the ordering invariant that the expanded path clears an existing
  minimum before applying a monitor-clamped size and installs its effective
  minimum afterward.
- Keep the current compact/expanded transition tests to prove collapse remains
  fixed-size and re-expansion remains bounded and resizable.

### 7.3 Verification

- Run focused frontend window-mode and configuration tests first.
- Run the full frontend check, lint, test, and production build gates.
- Run focused Rust window-mode tests, then the repository's Rust test and
  clippy gates applicable to the change.
- Inspect the browser preview at expanded desktop size and after collapsing to
  `400x640`, checking that navigation, window controls, and page content do not
  overlap.
- Build the Windows installer through the existing CI packaging path because
  local Smart App Control currently blocks the Tauri packaging toolchain.
- Do not execute the installer locally. Verify its build metadata and replace
  the existing desktop installer only after CI succeeds.

These tests do not invoke OrangeAPI local-write commands and must not read,
write, back up, or restore the user's real `config.toml`, `auth.json`, or CC
Switch data. If a later native smoke test can touch those paths, it must first
make a fresh backup, restore it afterward, and verify pre/post hashes.

## 8. Acceptance Criteria

- A packaged Windows or macOS launch opens directly at the expanded workbench
  size, subject to monitor work-area clamping.
- The sidebar is visible immediately and contains Home, API Configuration,
  Themes, and Settings.
- There is no visible compact-to-expanded startup resize or layout flash.
- Collapse switches to a fixed `400x640` window with no rail.
- Expand from compact restores a resizable, bounded workbench and the rail.
- Browser preview and packaged startup use the same initial mode.
- Existing navigation, API Configuration, installer, update, theme, and
  compact-mode behaviors remain operational.
- No verification step modifies the user's real Codex or CC Switch state.

## 9. Non-Goals

- Removing compact mode or its expand control.
- Remembering compact mode across process launches.
- Changing rail contents, order, styling, or API Configuration behavior.
- Modifying `E:\ForkProject\orangeapi`.
- Changing Codex installation, update, uninstall, configuration-write, restart,
  or CC Switch import logic.
