<!--
  Release-notes template. Copy to docs/releases/v<X.Y.Z>.md inside the
  version-bump PR; the unprivileged tag signal asks the default-branch
  release.yml to pick it up by tag name
  (and appends GitHub's auto-generated "What's Changed" + Full Changelog).
  If the file is missing the workflow falls back to a minimal install table,
  so a release is never published with an empty body.

  Style (融合 Deck 的双语成对行 + 我们自己的分发重点):
  - zh 在前,en 紧随成对出现;不堆营销词,写用户可感知的变化。
  - 要点 3-6 条;patch 版可以只有「修复」一组。
  - 不要写未核实的渠道/数字;镜像直链恒指向最新版,历史版本读者请用页面下方 Assets。
-->

<p align="center">
  <a href="https://codexapp.agentsmirror.com">
    <img src="https://raw.githubusercontent.com/Wangnov/Codex-App-Manager/main/assets/banner.svg" alt="Codex App Manager" width="100%">
  </a>
</p>

> 一句话概括这一版(zh)。
> One line on what this release does (en).

## ✨ 亮点 · Highlights

- **要点标题**:中文说明,落在用户可感知的行为变化上。
  English counterpart, written natively — not a translation artifact.

## 🐛 修复 · Fixes

- **修了什么**:之前的症状 → 现在的行为。
  What was broken → what happens now.

## 📦 安装与升级 · Install & Upgrade

**已经安装?** 打开应用即可收到本次更新——macOS 只下载版本间的增量,校验失败自动回滚。
**Already installed?** The app offers this update in-app — macOS pulls only the delta, with automatic rollback.

| 平台 · Platform | 下载 · Download(国内直连 · China-reachable) |
| --- | --- |
| macOS · Apple Silicon | [CodexAppManager_aarch64.dmg](https://codexapp.agentsmirror.com/manager/latest/CodexAppManager_aarch64.dmg) |
| macOS · Intel | [CodexAppManager_x86_64.dmg](https://codexapp.agentsmirror.com/manager/latest/CodexAppManager_x86_64.dmg) |
| Windows · x64 | [CodexAppManager_x64-setup.exe](https://codexapp.agentsmirror.com/manager/latest/CodexAppManager_x64-setup.exe) |
| Windows · ARM64 | [CodexAppManager_arm64-setup.exe](https://codexapp.agentsmirror.com/manager/latest/CodexAppManager_arm64-setup.exe) |

**Windows 签名状态（本版必须据实填写）:** 当前安装器没有 Authenticode 代码签名,首次运行可能出现 SmartScreen 提示;不得把 `.sig` / `latest.json` 的 Tauri updater 字节签名写成 Windows 发行者身份。SignPath Foundation 申请仍在审核。参见 [代码签名政策](https://github.com/Wangnov/Codex-App-Manager/blob/main/docs/code-signing-policy.md)与 [Windows signing and verification](https://github.com/Wangnov/Codex-App-Manager/blob/main/docs/windows-signing.md)。
**Windows signing status (replace with this release's verified facts):** The current installers are not Authenticode-signed, so SmartScreen may warn on first run. Never describe a Tauri updater signature in `.sig` / `latest.json` as Windows publisher identity. The SignPath Foundation application is pending. See the [code signing policy](https://github.com/Wangnov/Codex-App-Manager/blob/main/docs/code-signing-policy.md) and [Windows signing and verification](https://github.com/Wangnov/Codex-App-Manager/blob/main/docs/windows-signing.md).

**隐私 · Privacy:** [隐私政策 · Privacy policy](https://github.com/Wangnov/Codex-App-Manager/blob/main/docs/privacy.md)

**核验下载:** 本页 Assets 带有 `SHA256SUMS`;Windows 用 `Get-FileHash .\CodexAppManager_x64-setup.exe -Algorithm SHA256` 或替换为 ARM64 文件名,macOS 用 `shasum -a 256 CodexAppManager_aarch64.dmg`,再与 `SHA256SUMS` 比对。
**Verify downloads:** This release includes `SHA256SUMS` in Assets; on Windows run `Get-FileHash .\CodexAppManager_x64-setup.exe -Algorithm SHA256` or swap in the ARM64 filename, and on macOS run `shasum -a 256 CodexAppManager_aarch64.dmg`, then compare with `SHA256SUMS`.

```bash
# macOS · Homebrew
brew install --cask wangnov/tap/codex-app-manager
```

> 镜像直链恒指向**最新**版本;如需本页对应的历史版本,请使用下方 Assets。`.app.tar.gz` / `.sig` / `latest.json` 是自动更新器的工件,手动安装请选 `.dmg` / `.exe`。
> Mirror permalinks always resolve to the **latest** release — for this exact version use the assets below. `.app.tar.gz` / `.sig` / `latest.json` belong to the auto-updater; pick the `.dmg` / `.exe` for manual installs.
