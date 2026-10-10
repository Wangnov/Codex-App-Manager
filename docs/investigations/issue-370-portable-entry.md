# Issue #370: portable folder entry points

The official Windows executable needs package identity on its default startup
path in Codex 26.915 and later. The Manager already supplies the bundled CLI
through a child-process environment, but users could still double-click the
official executable in the portable folder and hit the bootstrap error.

New installs, updates and same-version repairs use this layout:

```text
Codex/
  Codex.exe                    native launcher; Start-menu target
  ChatGPT.exe                  compatibility launcher for existing root shortcuts
  LaunchCodex.exe               compatibility launcher
  codex-portable-launcher.txt   relative target: app\ChatGPT.exe (or app\Codex.exe)
  AppxManifest.xml              original package identity and entry metadata
  app/                         unchanged official executable, DLLs and resources
```

The launch configuration also records `require-cli=1` when the upstream payload
declares an app-contained core, or `require-cli=0` for legacy payloads. This keeps
CLI-less historical apps launchable. Old single-line configs retain their original
CLI-required behavior.

The existing staged directory swap commits this layout as one tree. Rollback
restores the complete previous tree, including a flat install's original EXE,
resources and launcher configuration. Ordinary Manager launches continue to
repair a flat install in place without moving its payload; users can update or
reinstall the same version to get the new folder entry points.

All root launchers resolve paths relative to themselves, set the GUI child's
working directory to `app/`, select that payload's bundled CLI, and forward
arguments without a shell. Official files and global environment settings are
unchanged. Detection and health checks resolve the official nested executable,
never a root launcher alias. A missing nested payload is treated as broken.

## Validation

- Native Windows launcher tests run all three root entry names after moving the
  folder to a path containing Chinese text, spaces, an apostrophe and `&`.
  They check CLI selection, working directory, registration-environment cleanup
  and exact forwarding of a URL-shaped argument containing quotes and spaces.
- Package fixtures check both legacy and rebranded MSIX layouts, preserve the
  official binaries byte-for-byte, and verify successful flat-to-nested updates.
- A failure after the new tree is installed restores the entire flat layout.
- Entry probing rejects leftover root launchers when the nested entry or its
  directory is missing. Launcher configuration rejects absolute paths/traversal.
- The launcher's PE imports still require no VC++ redistributable.
- CLI-less legacy fixtures start through every applicable root launcher in both
  flat and nested layouts; app-contained-core payloads still reject a missing CLI.
- Failed launcher/config replacement leaves the previous config intact; repair
  upgrades backward-compatible launchers before atomically replacing the config.
  Empty or malformed config cannot redirect detection to a root launcher alias.
- An isolated copy of the official `26.930.3930.0` payload in the nested layout
  passed native main-window startup checks both through the engine and through
  the root `Codex.exe` launcher with no inherited CLI override. SHA256 checks on
  its GUI EXE, ASAR and CLI matched the original installed files. No login state
  was copied; the lab's shared browser-host manifest change was restored after
  each test.

## Upstream integration boundaries

Read-only inspection of the installed official Codex `26.930.3930.0` payload
shows that Windows skips Electron's `setAsDefaultProtocolClient` call; the MSIX
manifest normally supplies that integration. Moving the payload therefore does
not by itself add a working Windows `codex://` registration to a portable install.
The Manager now registers a separate per-user `CodexAppManager.Portable` ProgID
and its `codex` URL capability, displayed as **Codex (Portable)** in Windows
Default apps. The registered command targets the root launcher and uses a
restricted URL argument mode: exactly one `codex:` URL, with no raw whitespace,
quotes, backslashes or control characters. Direct launcher argument forwarding
retains its previous behavior. Registration happens after the payload passes
the startup health check, and is repaired on Manager launch. Both uninstall
paths remove only Manager-owned registration and preserve other applications.
Repair refreshes legacy Apps & Features uninstall metadata before adding the
handler; registration is skipped if that refresh fails. Ownership conflicts
are checked before any registration writes, including the capability and
RegisteredApplications boundaries.
The Apps & Features command uses encoded PowerShell so quoted protocol commands
and paths with apostrophes survive the command-line transport.

Existing Windows defaults are preserved. An installed MSIX can remain the
default even when the portable handler is registered; users can select
**Codex (Portable)** in Settings when they want it to handle links. The common
`Software\Classes\codex` hint and Windows UserChoice are not modified. See
[Windows app defaults](https://learn.microsoft.com/en-us/windows/apps/develop/windows-integration/default-apps-platform).
The local OS activation probe still reached the installed MSIX. Actual activation
of the selected portable handler and its destination page await the manual
Default apps check; this is not yet recorded as passing.

The same payload's Chrome native-host lifecycle builds the host manifest from
`extensionHostPath` (the plugin's `extension-host.exe`) and resolves the CLI,
Node and node-repl through `resourcesPath`. It does not use the GUI executable
as the browser native host. Keeping the complete resources directory beside the
official executable preserves those relative paths; browser-extension requests
after login still need end-to-end verification. The isolated official window has
been signed in through ChatGPT, and its browser runtime registry points to the
nested payload's Node/resources and the isolated plugin host. A browser action
through the signed-in window is still pending.

Existing shortcuts or pins aimed at a root EXE retain their target paths. New
Start-menu shortcuts also carry the official stable process AppUserModelID,
`com.openai.codex`, so Windows can match the running GUI to the launcher shortcut.
A native Shell property-store test confirms this ID and the unchanged shortcut
target, including Chinese text, spaces, apostrophes and `&` in the path. The user
has created a pin from the isolated window; closing it and starting it through
the pin still needs a manual check. In-app relaunch also remains unverified.
Installation and cleanup unit fixtures skip real shell metadata; dedicated
registry tests use unique private subtrees and exercise both uninstall paths.

Issue #370 remains open for user-environment confirmation. Actual activation of
the selected portable protocol, reopening a new taskbar pin and a signed-in
browser action have not been confirmed by manual verification. Automated
regression, real x64/ARM64 MSIX extraction and installer checks have passed.
