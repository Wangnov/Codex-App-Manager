# Roadmap & 验收清单

> 配套 [`product-design.md`](./product-design.md)。状态：✅ 完成 · 🟡 进行中 · ⬜ 未开始。
> 当前版本 `v0.5.10`（见 [`docs/releases/`](./releases/)）。macOS 更新引擎、mirror 侧 appcast/delta 镜像、
> manager 自更新与分发均已上线；Windows 目前是「全量下载 + 关-换-重启」的 α 阶段，增量与签名仍在推进。

## 状态总览

| 工作流 | 状态 | 说明 |
|---|---|---|
| macOS 更新引擎（appcast→plan→download→verify→apply→gate→swap→rollback） | ✅ | 自 v0.1.13 起随包出厂的 BinaryDelta 已端到端可用；一版落后仅下 ~18MB delta（全量 406MB） |
| mirror 服务端：Sparkle appcast + zip + delta 镜像 | ✅ | `codex-app-mirror` 重写 appcast enclosure URL 指向镜像、原样保留 OpenAI EdDSA 签名，manager 端 `PROD_ARM64_APPCAST` / `PROD_X64_APPCAST` 消费 |
| mirror 服务端：manifest 契约（schemaVersion 2） | ✅ | [`manifest-contract.md`](./manifest-contract.md)；`sources.windows/macos` + `manager.payloads` 预留字段稳定 |
| manager 自更新 + 分发（latest.json / R2+IHEP 双活） | ✅ | `scripts/mirror-release.mjs` 做 R2 CAS 主链路 + IHEP 跟随，`release.yml` 自动签 `latest.json`；见 [`release.md`](./release.md) |
| macOS 纳管 / provenance UX | ✅ | managed/external/none 分类 + 显式同意纳管 |
| Windows 全链路（识别→侧载/便携→运行中替换→回滚） | 🟡 | α 阶段已上线且持续修复至 v0.5.10（MSIX 侧载失败自动回退便携、启动校验、日志恢复等）；文件级/块级增量（β/γ）未开始 |
| Windows 便携直启入口（`ChatGPT.exe` 双击） | ⬜ | Codex 26.915 起需要包身份，双击官方 EXE 仍失败；[#370](https://github.com/Wangnov/Codex-App-Manager/issues/370) 跟踪 |
| Windows 块级增量更新（γ，zsync/Range 复用） | ⬜ | 见下 §3；当前 Windows 更新是全量重下 |
| Windows Authenticode 签名 | ⬜ | SignPath Foundation 申请（2026-07-11 提交）不再继续推进，正在评估备选签名服务商；当前无 Authenticode 签名 |
| 上游兼容性监测流水线 | ⬜ | 见下 §4；当前只有 15 分钟探测触发镜像发布，无「新版发布后自动探测功能是否被破坏」的诊断/修复闭环 |
| Cargo workspace 整合 | ⬜ | `src-tauri` / 三个 engine crate 仍各自独立 `Cargo.lock`，未合并为单一 workspace |
| `~/.codex` 边界 | ⬜ | 仅卸载时保留/清除，其余预留不做 |

---

## 1. macOS 更新引擎与 mirror 侧 ✅

已上线并在生产验证：
- `codex-mac-engine`：`mac_plan_update` → `download_and_verify`（EdDSA 钉死官方公钥）→ `apply_delta` / `unpack_app_zip` → `codesign` gate（Team `2DC432GLL2`，Notarized）→ 退出→同卷原子替换→健康检查→`relaunch`/`rollback`。
- `codex-app-mirror` 侧：`build-appcast.sh` 重写 enclosure 指向镜像、保留 OpenAI 原始 `sparkle:edSignature`；`.delta` 与全量 `.zip` 一并同步、按最近窗口 prune。
- manager 消费镜像 appcast（`PROD_ARM64_APPCAST`/`PROD_X64_APPCAST`），官方 appcast 仅作不可达兜底。
- 自 v0.1.13（BinaryDelta 随包出厂）起持续验证，至 v0.5.10 稳定运行；[`macos-delta-updates.md`](./macos-delta-updates.md) 记录构建期两个前置条件（vendor BinaryDelta + 内嵌助手签名）。

## 2. manager 自更新 + 分发 ✅

- Tauri updater 接 `latest.json`（minisign/Tauri 签名），`scripts/mirror-release.mjs` 把安装包与 `latest.json` 同步到 R2（主链路，CAS 防降级）与 IHEP（跟随，失败可回滚），见 [`release.md`](./release.md)。
- GitHub Release + agentsmirror 镜像 + `SHA256SUMS` 独立核验；winget（`Wangnov.CodexAppManager`）稳定版自动提交。
- macOS Developer ID 签名 + 公证已生效；Windows 侧见 §5（Authenticode 未完成）。

## 3. Windows 全链路 🟡

α 阶段（全量下载 + 关-换-重启）已上线并在多个版本中修复实际问题：
- v0.5.7/v0.5.8：新版 Codex（26.915.31029）便携启动失败修复，`LaunchCodex.exe` 启动器落地，MSIX BlockMap 逻辑路径解码修复。
- v0.5.9：MSIX 启动失败误判为成功的问题修复，恢复"侧载失败自动回退便携"路径。
- v0.5.10：历史版本列表加载超时、运行中删除日志恢复、便携恢复结果展示简化。
- v0.5.10 后：系统代理（WinINET）在 curl 直连模式下的识别修复（#369）。

未开始（见 [`product-design.md` §9](./product-design.md)）：
- **β（文件级增量）**：比对已装文件树哈希与新版 `*.files.json`，只下哈希变化的文件，省去稳定的 Chromium 框架/`node.exe`；MSIX 本质是 zip，按中央目录 Range 抽取条目。
- **γ（块级增量）**：对 `app.asar` 等易变大文件做块级/zsync 复用，需要 mirror 侧发布 zsync 控制数据并支持 Range 托管。
- **验收标准**（未达成）：Windows 更新体积与耗时接近 macOS delta 的量级；文件级/块级复用在增量失败时能回退全量，不引入新的启动失败模式。

## 4. Windows 便携直启入口 ⬜

[#370](https://github.com/Wangnov/Codex-App-Manager/issues/370)：Codex 26.915 起官方 `ChatGPT.exe` 默认启动路径需要 MSIX 包身份，便携安装目录下双击该 EXE 仍会报「该进程没有程序包标识符」。当前 Manager、开始菜单快捷方式、`LaunchCodex.exe` 均可正常启动，只有直接双击上游 EXE 这一入口未覆盖。待评估方向：把官方 payload 放进子目录、启动器放在便携根目录并在更新/回滚时迁移已有安装，同时确认 `codex://` 协议处理器、Chrome 原生消息宿主等依赖 `process.execPath` 的路径在改动后仍可用。

## 5. Windows Authenticode 签名 ⬜

- SignPath Foundation 免费签名申请已于 2026-07-11 提交，PR #180 落地了申请前置文档；但该路线不再继续推进，正在评估备选签名服务商。
- 仓库现有的 PFX 签名脚手架（`scripts/sign-windows-authenticode.ps1` 等）是可选占位路径，不等于任何正式签名集成；证书未配置时签名步骤跳过、校验非阻塞。
- 当前风险披露与核验方式（`SHA256SUMS`、Tauri updater 签名）见 [`windows-signing.md`](./windows-signing.md) 与 [`code-signing-policy.md`](./code-signing-policy.md)，选定新供应商后需要单独 PR 接入并更新这两份文档。

## 6. 上游兼容性监测流水线 ⬜

现状：`codex-app-mirror` 每 15 分钟探测上游 Store/APT 元数据变化，仅用于判断"是否要发一个新的镜像版本"，探测不到就跳过；没有任何环节验证新版发布后 Manager 的安装/更新/启动链路是否仍然工作。
目标（未开始，需要单独设计）：
- 在检测到上游新版本的分钟级窗口内，自动跑一轮"安装/更新/启动"最小回归（至少 Windows MSIX 侧载 + macOS delta 应用两条关键路径）。
- 失败时自动收集诊断（安装日志、启动失败弹窗文案、PE/包签名状态等）并归档，而不是等用户报 issue。
- 形成"探测到破坏 → 收集诊断 → 定位修复 → 发布"闭环，缩短从上游变更到用户可见修复之间的时间。

## 7. Cargo workspace 整合 ⬜

`src-tauri/Cargo.toml`、`crates/codex-mac-engine`、`crates/codex-win-engine`、`crates/codex-theme-engine` 目前各自维护独立 `Cargo.lock`，不是同一个 workspace。合并为单一 workspace（共享 lockfile、统一依赖版本、减少 CI 里重复编译）尚未开始，需要评估对发版流程第 1 步"改 5 个文件 6 处版本号"（含 `src-tauri/Cargo.lock` 的定向编辑限制）的影响。

## 8. 横切事项

- [x] provenance store（bundle 之外）：记录 managed/external + 来源，驱动纳管。
- [x] 纳管 UX：检测外部安装 → 显式同意接管流。
- [ ] `~/.codex` 边界：仅卸载时"保留/清除"，其余预留不做。

---

## 已交付的可运行验证（存量，持续有效）

- `cargo test -p codex-mac-engine` / manager `cargo test`：appcast/plan/verify/swap、full-zip 解包分支持续绿。
- `cargo run -p codex-mac-engine --bin mac_plan / mac_fetch / mac_rehearse / mac_live_test`：appcast 出 delta 计划、真实 delta 下载 + EdDSA 验签、沙盒彩排、真实 `/Applications` gate→替换→relaunch，均已跑通。
- `npm run build` / `cargo clippy --all-targets -- -D warnings`：manager 前端与 manager+engine 两 crate 保持干净。
- Windows 侧：`win-installer-check.yml` 覆盖 x64 install→launch→upgrade→uninstall 冒烟 + Authenticode 探测（非阻塞）；ARM64 仅交叉构建 + PE machine 诊断，真机运行验证仍需人工清单（见 [`windows-signing.md`](./windows-signing.md)）。
- `codex review --base <slice>`：作为发版前收尾链路的标准步骤，迭代到无阻断意见后再开 PR/合并。
