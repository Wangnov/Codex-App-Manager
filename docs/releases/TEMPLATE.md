<!--
  Release-notes template. Copy to docs/releases/v<X.Y.Z>.md inside the
  version-bump PR; release.yml picks it up by tag name when the tag is pushed
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

```bash
# macOS · Homebrew
brew install --cask wangnov/tap/codex-app-manager
```

> 镜像直链恒指向**最新**版本;如需本页对应的历史版本,请使用下方 Assets。`.app.tar.gz` / `.sig` / `latest.json` 是自动更新器的工件,手动安装请选 `.dmg` / `.exe`。
> Mirror permalinks always resolve to the **latest** release — for this exact version use the assets below. `.app.tar.gz` / `.sig` / `latest.json` belong to the auto-updater; pick the `.dmg` / `.exe` for manual installs.
