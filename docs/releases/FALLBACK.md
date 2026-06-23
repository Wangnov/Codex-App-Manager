<p align="center">
  <a href="https://codexapp.agentsmirror.com">
    <img src="https://raw.githubusercontent.com/Wangnov/Codex-App-Manager/main/assets/banner.svg" alt="Codex App Manager" width="100%">
  </a>
</p>

## 📦 安装与升级 · Install & Upgrade

**已经安装?** 打开应用即可收到本次更新——macOS 只下载版本间的增量,校验失败自动回滚。
**Already installed?** The app offers this update in-app — macOS pulls only the delta, with automatic rollback.

| 平台 · Platform | 下载 · Download(国内直连 · China-reachable) |
| --- | --- |
| macOS · Apple Silicon | [CodexAppManager_aarch64.dmg](https://codexapp.agentsmirror.com/manager/latest/CodexAppManager_aarch64.dmg) |
| macOS · Intel | [CodexAppManager_x86_64.dmg](https://codexapp.agentsmirror.com/manager/latest/CodexAppManager_x86_64.dmg) |
| Windows · x64 | [CodexAppManager_x64-setup.exe](https://codexapp.agentsmirror.com/manager/latest/CodexAppManager_x64-setup.exe) |
| Windows · ARM64 | [CodexAppManager_arm64-setup.exe](https://codexapp.agentsmirror.com/manager/latest/CodexAppManager_arm64-setup.exe) |

**Windows 签名状态:** `CodexAppManager_x64-setup.exe` / `CodexAppManager_arm64-setup.exe` 当前没有 Authenticode 代码签名,首次运行可能出现 SmartScreen 提示;`.sig` / `latest.json` 里的 Tauri updater 签名只用于应用内自更新的字节校验,不代表 Windows 发行者信任。详情见 [Windows signing and verification](https://github.com/Wangnov/Codex-App-Manager/blob/main/docs/windows-signing.md)。
**Windows signing status:** `CodexAppManager_x64-setup.exe` / `CodexAppManager_arm64-setup.exe` are not Authenticode-signed yet, so SmartScreen may warn on first run; the Tauri updater signature in `.sig` / `latest.json` verifies in-app update bytes only and is not Windows publisher trust. See [Windows signing and verification](https://github.com/Wangnov/Codex-App-Manager/blob/main/docs/windows-signing.md).

**核验下载:** 本页 Assets 带有 `SHA256SUMS`;Windows 用 `Get-FileHash .\CodexAppManager_x64-setup.exe -Algorithm SHA256` 或替换为 ARM64 文件名,macOS 用 `shasum -a 256 CodexAppManager_aarch64.dmg`,再与 `SHA256SUMS` 比对。
**Verify downloads:** This release includes `SHA256SUMS` in Assets; on Windows run `Get-FileHash .\CodexAppManager_x64-setup.exe -Algorithm SHA256` or swap in the ARM64 filename, and on macOS run `shasum -a 256 CodexAppManager_aarch64.dmg`, then compare with `SHA256SUMS`.

```bash
# macOS · Homebrew
brew install --cask wangnov/tap/codex-app-manager
```

> 镜像直链恒指向**最新**版本;如需本页对应的历史版本,请使用下方 Assets。`.app.tar.gz` / `.sig` / `latest.json` 是自动更新器的工件,手动安装请选 `.dmg` / `.exe`。
> Mirror permalinks always resolve to the **latest** release — for this exact version use the assets below. `.app.tar.gz` / `.sig` / `latest.json` belong to the auto-updater; pick the `.dmg` / `.exe` for manual installs.
