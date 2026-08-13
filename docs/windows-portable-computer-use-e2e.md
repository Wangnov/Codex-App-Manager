# Windows portable Computer Use E2E

This is the release gate for the portable MSIX logical-path repair. A green
manager build or a successful `ChatGPT.exe` launch is not sufficient: the
bundled Computer Use Node runtime must resolve its scoped modules, and the
interactive Computer Use path must survive a cold restart.

## Automated package matrix

[`windows-portable-msix-e2e.yml`](../.github/workflows/windows-portable-msix-e2e.yml)
downloads the current official x64 and ARM64 MSIX assets on native Windows
runners. For each package it:

1. extracts the portable payload using the production extractor;
2. verifies `@oai/sky`, `@statsig/client-core`, and
   `$_StatsigGlobal.js` use their logical `AppxBlockMap.xml` names;
3. rejects leaked `%40oai`, `%40statsig`, and `%24_StatsigGlobal.js`
   paths; and
4. executes the architecture-matched bundled `cua_node\bin\node.exe` and
   resolves both scoped modules.

The workflow is a required release signal for changes to the portable
extractor, but it cannot prove interactive WGC, UI Automation, or input.

## Interactive Windows acceptance

Use an unlocked interactive Windows x64 machine or VM. Windows Server 2022
matches Issue #260; Windows 11 is also useful. Create a VM checkpoint or backup
first, and fully quit Codex before replacing it.

1. Install the PR build of Codex App Manager.
2. Open **Choose install version**.
3. Select the current version marked **Repair by reinstalling**. This action is
   offered only for a managed portable installation.
4. Confirm **Download and repair**, wait for the atomic replacement to finish,
   and open Codex.
5. From the repository checkout, run the non-mutating runtime probe:

   ```powershell
   npm ci
   npm run build
   cargo run --manifest-path src-tauri/Cargo.toml `
     --example win_real_smoke -- validate-portable-runtime
   ```

6. In Codex, exercise Computer Use against an unsaved Notepad test window:
   native pipe ready, application/window enumeration, screenshot, UI
   Automation, and benign keyboard/mouse input must all succeed.
7. Fully quit Codex, start it again, and repeat the enumeration plus screenshot
   checks. This cold restart must also pass.

The probe emits JSON containing the resolved `@oai/sky` and
`@statsig/client-core` paths. Attach that JSON and the manual acceptance result
to the Draft PR before marking it ready.

The runtime probe reads the install root saved by Codex App Manager. To inspect
an isolated custom tree instead, pass its exact path after the command, for
example `validate-portable-runtime D:\Codex`.

## Disposable-VM full lifecycle

`force-portable-cycle` installs the current real MSIX as portable, repeats the
same-version replacement, validates the bundled Computer Use runtime after
both installs, uninstalls the portable tree, and restores the final MSIX
installation:

```powershell
npm ci
npm run build
cargo run --manifest-path src-tauri/Cargo.toml `
  --example win_real_smoke -- force-portable-cycle
```

Run this mutation-heavy cycle only in a disposable VM. It preserves the Codex
user-data directory, but it intentionally changes the installed application
and package registration while testing rollback and cleanup paths.
