# Default Expanded Workbench Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Start Windows, macOS, and browser builds in the expanded `1100x720` workbench with the sidebar visible on the first frame while preserving the explicit compact-mode transition.

**Architecture:** Keep one fixed startup policy across three layers: the Tauri window config creates an expanded native frame, native construction normalizes that hidden frame through the existing window-mode engine, and HTML plus React both start with `expanded`. Runtime switching continues through the existing `set_window_mode` command, with a small ordering correction so an existing minimum cannot block monitor-aware startup clamping.

**Tech Stack:** Tauri v2, Rust 2021, React 19, TypeScript 6, Vitest/Testing Library, Vite, GitHub Actions NSIS packaging.

---

## File Map

- Modify `src/app/windowMode.test.tsx`: regression coverage for expanded-first rendering and the preserved collapse/re-expand path.
- Modify `src/app/windowMode.tsx`: make React's startup mode expanded and correct compact-first documentation.
- Modify `src/app/securityConfig.test.ts`: contract coverage for the initial HTML marker and packaged Tauri window properties.
- Modify `index.html`: activate expanded CSS before the first React effect.
- Modify `src/app/styles.css`: correct the window-mode documentation comment; no visual token changes.
- Modify `src-tauri/tauri.conf.json`: declare the expanded initial native frame.
- Modify `src-tauri/src/app/window_mode.rs`: define the native startup mode, clear stale minimum size before expanded sizing, and cover the startup policy.
- Modify `src-tauri/src/lib.rs`: normalize the newly built hidden main window before it can be shown.

### Task 1: Lock and implement the renderer startup contract

**Files:**
- Modify: `src/app/windowMode.test.tsx`
- Modify: `src/app/securityConfig.test.ts`
- Modify: `src/app/windowMode.tsx`
- Modify: `index.html`
- Modify: `src/app/styles.css`

- [ ] **Step 1: Write failing tests for expanded-first rendering**

Replace the compact-first assertion and transition helpers in
`src/app/windowMode.test.tsx` with an expanded-first contract:

```tsx
async function collapse() {
  fireEvent.click(screen.getByRole("button", { name: /collapse workspace/i }));
  await waitFor(() =>
    expect(document.documentElement.dataset.windowMode).toBe("compact"),
  );
}

async function expand() {
  fireEvent.click(screen.getByTitle(/expand workspace/i));
  await waitFor(() =>
    expect(document.documentElement.dataset.windowMode).toBe("expanded"),
  );
}

it("starts expanded: mode stamped on <html>, rail shown, collapse offered", () => {
  render(<Harness />);
  expect(document.documentElement.dataset.windowMode).toBe("expanded");
  expect(screen.getByRole("navigation")).toBeInTheDocument();
  expect(screen.queryByTitle(/expand workspace/i)).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: /collapse workspace/i })).toBeInTheDocument();
});

it("collapse hides the rail and re-expansion restores it with size memory", async () => {
  render(<Harness />);
  await collapse();
  expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
  await expand();
  expect(screen.getByRole("navigation")).toBeInTheDocument();
  expect(JSON.parse(localStorage.getItem("cam.windowSize.expanded") ?? "null")).toEqual({
    width: 1100,
    height: 720,
  });
});
```

Update other tests that call `expand()` so they begin from the already-expanded
state, and make the collapse test call `collapse()` directly.

Extend `src/app/securityConfig.test.ts` with the static first-paint contract:

```ts
import indexHtml from "../../index.html?raw";

it("stamps expanded mode before the renderer's first paint", () => {
  expect(indexHtml).toContain('<html lang="zh-CN" data-window-mode="expanded">');
});
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```powershell
$env:Path='F:\Git\bin;'+$env:Path
npx vitest run src/app/windowMode.test.tsx src/app/securityConfig.test.ts
```

Expected: FAIL because `WindowModeProvider` still initializes to `compact` and
`index.html` has no `data-window-mode="expanded"` attribute.

- [ ] **Step 3: Implement the renderer startup state**

In `src/app/windowMode.tsx`, keep expanded-size persistence but initialize the
mode as expanded:

```tsx
/** Remembered expanded size (logical px, JSON `{width,height}`). Startup mode
 *  itself is deliberately fixed to expanded; compact remains a per-session
 *  posture selected through the rail. */
const LS_SIZE_KEY = "cam.windowSize.expanded";

export function WindowModeProvider({ children }: { children: ReactNode }) {
  const [mode, setModeState] = useState<WindowMode>("expanded");
```

In `index.html`, change the opening element to:

```html
<html lang="zh-CN" data-window-mode="expanded">
```

In `src/app/styles.css`, change the mode comment to identify expanded as the
startup default while leaving all selectors and layout values unchanged.

- [ ] **Step 4: Run the focused tests and verify GREEN**

Run:

```powershell
$env:Path='F:\Git\bin;'+$env:Path
npx vitest run src/app/windowMode.test.tsx src/app/securityConfig.test.ts
```

Expected: both files pass, the initial test finds the navigation rail, and the
collapse/re-expand test proves compact mode remains reachable.

- [ ] **Step 5: Commit the renderer change**

```powershell
git add -- index.html src/app/windowMode.tsx src/app/windowMode.test.tsx src/app/securityConfig.test.ts src/app/styles.css
git commit -m "fix: start renderer in expanded workbench"
```

### Task 2: Lock and implement the native startup contract

**Files:**
- Modify: `src/app/securityConfig.test.ts`
- Modify: `src-tauri/tauri.conf.json`
- Modify: `src-tauri/src/app/window_mode.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write a failing packaged-window configuration test**

Extend the `TauriConfig` test shape in `src/app/securityConfig.test.ts`:

```ts
interface TauriWindowConfig {
  label: string;
  create?: boolean;
  width?: number;
  height?: number;
  minWidth?: number;
  minHeight?: number;
  resizable?: boolean;
  shadow?: boolean;
}

interface TauriConfig {
  app: {
    windows: TauriWindowConfig[];
    security: { csp: string; devCsp: string };
  };
}
```

Add the packaged startup assertion:

```ts
it("declares the main window as an expanded workbench", () => {
  const config = tauriConfigJson as TauriConfig;
  expect(config.app.windows.find((window) => window.label === "main")).toMatchObject({
    width: 1100,
    height: 720,
    minWidth: 960,
    minHeight: 640,
    resizable: true,
    shadow: true,
  });
});
```

In `src-tauri/src/app/window_mode.rs`, add a Rust unit test before defining the
constant:

```rust
#[test]
fn startup_mode_is_expanded() {
    assert_eq!(INITIAL_WINDOW_MODE, WindowMode::Expanded);
}
```

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```powershell
$env:Path='F:\Git\bin;'+$env:Path
npx vitest run src/app/securityConfig.test.ts
cargo test --manifest-path src-tauri/Cargo.toml app::window_mode::tests::startup_mode_is_expanded
```

Expected: Vitest FAILS because Tauri still declares `400x640`; Rust compilation
FAILS because `INITIAL_WINDOW_MODE` does not exist.

- [ ] **Step 3: Declare the expanded native frame**

Change the main window in `src-tauri/tauri.conf.json` to:

```json
"width": 1100,
"height": 720,
"minWidth": 960,
"minHeight": 640,
"resizable": true,
"decorations": false,
"transparent": true,
"visible": false,
"center": true,
"shadow": true
```

In `src-tauri/src/app/window_mode.rs`, define the startup policy beside the
size constants:

```rust
pub const INITIAL_WINDOW_MODE: WindowMode = WindowMode::Expanded;
```

Update the module comment so expanded is the launch form factor and compact is
an explicit runtime posture.

- [ ] **Step 4: Make expanded sizing safe with a declarative minimum**

Before the `match mode` block in `apply_window_mode`, clear any existing
minimum for both transitions:

```rust
window
    .set_min_size(None::<LogicalSize<f64>>)
    .map_err(|e| win_err("min size", e))?;

match mode {
    WindowMode::Compact => {
        window
            .set_size(LogicalSize::new(logical_w, logical_h))
            .map_err(|e| win_err("resize", e))?;
```

Remove the now-duplicate compact-only `set_min_size(None)` call. The expanded
branch continues to install `min(EXPANDED_MIN_SIZE, applied_size)` after the
normalized size has been applied.

- [ ] **Step 5: Normalize the hidden window during construction**

In `src-tauri/src/lib.rs`, after Windows accelerator configuration and before
returning from `build_main_window`, apply the startup policy:

```rust
    crate::app::window_mode::apply_window_mode(
        app.handle(),
        crate::app::window_mode::INITIAL_WINDOW_MODE,
        None,
        None,
    )
    .map_err(|error| {
        tauri::Error::Io(std::io::Error::other(format!(
            "failed to establish initial window mode: {error}"
        )))
    })?;
```

Keep the existing Windows WebView2 accelerator safety call immediately after
the builder and before this size normalization. On non-Windows, retain the
existing `let _ = window` only if the variable would otherwise be unused.

- [ ] **Step 6: Run focused tests and verify GREEN**

Run:

```powershell
$env:Path='F:\Git\bin;'+$env:Path
npx vitest run src/app/securityConfig.test.ts
cargo test --manifest-path src-tauri/Cargo.toml app::window_mode::tests
cargo check --manifest-path src-tauri/Cargo.toml
```

Expected: all commands pass; the configuration test sees the expanded native
frame and Rust confirms the initial mode plus monitor clamping behavior.

- [ ] **Step 7: Commit the native change**

```powershell
git add -- src/app/securityConfig.test.ts src-tauri/tauri.conf.json src-tauri/src/app/window_mode.rs src-tauri/src/lib.rs
git commit -m "fix: open native window in expanded mode"
```

### Task 3: Verify behavior, review, and produce the installer

**Files:**
- Verify only: all modified files and build outputs
- Deliver: `E:\windows\Desktop\Codex App Manager_0.5.0_x64-setup.exe`

- [ ] **Step 1: Protect real local configuration before native verification**

Do not run OrangeAPI write or restart commands. If any native smoke command can
invoke provider writes, first create a fresh timestamped backup outside
`~/.codex` for these exact targets and record SHA-256 hashes:

```text
C:\Users\dabin\.codex\config.toml
C:\Users\dabin\.codex\auth.json
C:\Users\dabin\AppData\Roaming\wangnov\codexappmanager\data
```

After the smoke, restore the backup and require matching hashes. Browser,
Vitest, Cargo unit tests, and CI packaged smoke do not call these write commands
and therefore must not create or alter the backup.

- [ ] **Step 2: Run all frontend gates**

Run:

```powershell
$env:Path='F:\Git\bin;'+$env:Path
npm run check
npm run lint
npm run test
npm run build
```

Expected: TypeScript and ESLint exit zero, all Vitest tests pass, and Vite emits
the production renderer with its entry check passing.

- [ ] **Step 3: Run all Rust gates**

Run:

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --workspace
cargo test --manifest-path crates/codex-mac-engine/Cargo.toml --all-targets
cargo test --manifest-path crates/codex-win-engine/Cargo.toml --all-targets
cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --all-targets
```

Expected: formatting, clippy, and every workspace/engine test exit zero.

- [ ] **Step 4: Inspect both window modes in a real browser**

Start Vite on an unused loopback port, open the page at `1100x720`, and assert
with browser automation that `data-window-mode` is `expanded`, one navigation
rail is visible, and its labels are Home, API Configuration, Themes, Settings.
Capture an expanded screenshot, click Collapse workspace, resize the browser
viewport to `400x640`, and assert the rail is absent and the expand control is
visible. Click Expand workspace, restore `1100x720`, and assert the rail returns.

Expected: no overlaps, clipped labels, blank content, or console errors in
either mode.

- [ ] **Step 5: Run the required code-review loop**

Run:

```powershell
codex review --uncommitted
```

After the implementation commits, run the repository-required branch review:

```powershell
codex review --base main
```

Expected: no findings. Fix any finding with a failing regression test where
applicable, rerun the relevant gates, and repeat review until clean.

- [ ] **Step 6: Push and verify cross-platform CI**

Push `codex/orangeapi-api-config` to `fork`. Verify PR 245's required Frontend,
Rust Windows, and Rust macOS checks. The changes to `src-tauri/**` also trigger
the Windows NSIS installer check; require its build, architecture validation,
install/launch/uninstall packaged smoke, and artifact upload to pass.

- [ ] **Step 7: Download and validate the Windows installer**

Download the `nsis-installer` artifact from the successful workflow run into a
fresh temporary directory. Require exactly one non-empty
`Codex App Manager_0.5.0_x64-setup.exe`, verify its PE machine is x64 with
`scripts/windows-pe-arch.ps1`, and compute its SHA-256 hash.

Do not execute the downloaded installer on this workstation because the user
requested that real local state remain untouched.

- [ ] **Step 8: Replace the desktop installer only after validation**

Copy the validated installer over:

```text
E:\windows\Desktop\Codex App Manager_0.5.0_x64-setup.exe
```

Recompute the destination SHA-256 and require it to equal the downloaded
artifact hash. Report the workflow URL, installer path, byte size, and SHA-256.

- [ ] **Step 9: Final completion audit**

Compare current evidence against every acceptance criterion in
`docs/superpowers/specs/2026-07-30-default-expanded-workbench-design.md` and the
user's packaging/state-safety requirements. Do not claim completion unless the
source tests, full gates, browser evidence, Windows/macOS CI, packaged smoke,
and desktop hash all prove the requested result.
