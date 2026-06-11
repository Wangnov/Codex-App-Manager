# CLAUDE.md — Agent 作业指南

Codex App Manager:官方 OpenAI Codex 桌面应用的安装/更新/卸载管理器(Tauri v2)。
前端 `src/`,Rust 引擎 `crates/` + `src-tauri/`,分发 Worker `cloudflare/`,官网 `website/`。

## 质量门与合并

- main 受保护:禁 force push、必须走 PR,必需检查 = Frontend + Rust (macos/windows)。
- 实质代码改动的收尾链路:`codex review --uncommitted`(或 `--base main`)**迭代到无意见** → PR → `gh pr merge --squash`。
- 提交信息用英文 conventional 风格(`feat:` / `fix:` / `docs:` / `chore(release):`)。

## 发版流程(tag 驱动)

1. **bump 版本号,5 个文件 6 处**:`package.json`、`package-lock.json`(顶层 + `packages[""]` 两处)、
   `src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`(只改
   `codex-app-manager` 那个 `[[package]]` 块——⚠️ lock 里 `winapi-util` 等依赖也是 `0.1.x`,
   **严禁全局替换**)。
2. **同一个发版 PR 里写 release note**:新增 `docs/releases/v<X.Y.Z>.md`。
   - 结构与双语风格照 `docs/releases/TEMPLATE.md`(banner 头图 → 一句话双语主旨 →
     ✨ 亮点 / 🐛 修复 双语成对行 → 安装升级表)。
   - 素材来源:`git log <上个tag>..HEAD` + **读关键 diff 还原细节**(squash 提交一句话
     背后常有数百行变更);只写用户可感知的变化,内部重构不写。
   - **事实核查**:渠道、数字必须核实——例:winget(`Wangnov.CodexAppManager`)在
     microsoft/winget-pkgs 合并前**不存在**,不要写 `winget install`。
   - 安装表、镜像直链、更新器工件说明是模板固化内容,不要改写;Full Changelog 由
     release.yml 自动追加,不要手写。
3. PR 标题 `chore(release): bump version to <X.Y.Z>`。bump PR 会额外触发**非必需**的
   `nsis` 检查(~8min,期间 PR 状态 UNSTABLE)——等它也绿再合并。
4. squash 合并后,在 main HEAD 打 annotated tag `v<X.Y.Z>` 并 push。`release.yml`
   自动:三平台构建 → 签名/公证 → R2 + IHEP 镜像同步(失败会阻断发布)→ GitHub
   Release(正文取 `docs/releases/<tag>.md`,缺失回退 `FALLBACK.md` 并告警,同时
   追加自动 What's Changed)。
5. 已知噪音:发布后 dispatch 的 winget workflow 在 winget-pkgs 首包 PR 合并前每次都
   fail,非回归;README 下载链接是 `/manager/latest/*` 永久直链,发版**无需**改 README。

## website/ 子工程(官网,codexapp.agentsmirror.com)

- 独立 npm 工程,**不要在仓库根目录为它装依赖**(会污染主工程 package.json)。
- 文案唯一来源 `website/src/locales/{zh,en}.ts`;改文案后重跑 `npm run fonts`
  (中文显示字体按用字子集化,缺字会回退系统字体)。
- 素材管线:`assets/raw/`(git-ignored,AI 生成)→ `npm run images` → `public/img/`。
- 部署:`cd website && npm run build && npx wrangler deploy`。zone 路由:
  `/manager/*` → 本仓库下载路由器,`/latest/*` → mirror 仓库,`/*` → 官网,互不抢路。
- README banner(`assets/banner.svg`)由 `node website/scripts/readme-banner.mjs`
  再生;官网视觉资产更新后记得重新生成。
