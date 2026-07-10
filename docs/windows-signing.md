# Windows signing and verification

This project currently ships a Windows NSIS installer without Authenticode code
signing. The installer is still published through GitHub Releases and the
agentsmirror download mirror, and every release includes `SHA256SUMS` so users
can verify the bytes they downloaded.

CI already prepares the Authenticode **path** (sign script, verify script, optional
release step, packaged lifecycle smoke). Certificate secrets are optional and
non-blocking until budget/demand justify an OV/EV cert.

## 中文

### 当前状态

- macOS 构建已经使用 Developer ID 签名并完成 Apple 公证。
- Windows 安装器 `CodexAppManager_x64-setup.exe` / `CodexAppManager_arm64-setup.exe` 当前没有 Authenticode 代码签名。
- Windows 应用内自更新包带有 Tauri updater 签名,用于校验下载字节没有被篡改。
- Windows 首次手动运行安装器时可能出现 SmartScreen 提示;这是预期风险,不是更新器签名失效。
- CI 已接入安装包冒烟(x64:`install → launch → upgrade → uninstall`)与 Authenticode 探测;证书未配置时签名步骤跳过且校验为非阻塞。

### 三个概念

**Tauri updater 签名:** `latest.json` 里的 `signature` 对安装包字节签名。它保护应用内自更新下载,确保镜像或网络传输没有改包。它不参与 Windows 系统的发行者信任判断,也不能消除 SmartScreen 提示。实现:`npx tauri signer sign` + `TAURI_SIGNING_PRIVATE_KEY`。

**Authenticode 代码签名:** Windows 对 PE 文件和安装器使用的代码签名体系。拥有 OV 或 EV 证书后,安装器可以显示发行者身份,并更容易建立系统信任。本项目当前还没有给 Windows 安装器做 Authenticode 签名。实现路径见下文「CI / 发布管线」。

**SmartScreen 信誉:** Microsoft Defender SmartScreen 会结合签名身份、下载量、历史信誉和风险信号做拦截判断。EV 证书通常能更快建立信誉;OV 证书和未签名分发都需要累积信誉,未签名安装器首次运行更容易被提示。

### 如何核验下载

1. 从 [GitHub Releases](https://github.com/Wangnov/Codex-App-Manager/releases/latest) 或 agentsmirror 镜像下载对应安装包。
2. 从同一个 release 的 Assets 下载 `SHA256SUMS`。
3. 在本机计算哈希并与 `SHA256SUMS` 中的同名文件比对。

Windows PowerShell:

```powershell
Get-FileHash .\CodexAppManager_x64-setup.exe -Algorithm SHA256
Get-FileHash .\CodexAppManager_arm64-setup.exe -Algorithm SHA256
```

macOS:

```bash
shasum -a 256 CodexAppManager_aarch64.dmg
shasum -a 256 CodexAppManager_x86_64.dmg
```

如果哈希不一致,不要运行该文件,请重新下载或在 issue 中反馈下载来源和文件名。

### 分发渠道与成本评估

**GitHub Releases + agentsmirror + SHA256SUMS:** 这是当前主渠道。优点是透明、可回溯、可独立核验;缺点是 Windows 无 Authenticode 时仍可能触发 SmartScreen。

**winget:** `Wangnov.CodexAppManager` 已在 microsoft/winget-pkgs 中可用,本仓库会在稳定版发布后自动提交新版本。winget 可接受未签名 NSIS 安装器,但新增架构或元数据变化仍可能触发人工审查。

**Microsoft Store / Partner Center:** 这是中期可选路径,需要开发者账号、MSIX 打包和商店审核。优点是用户信任更强,缺点是流程和维护成本更高。

**OV / EV 代码签名证书:** 证书不是当前发布阻塞项。后续可在 Windows 下载量、企业用户需求或支持成本达到明确阈值后重新评估。EV 成本更高但信誉建立更快;OV 成本较低但仍需累积 SmartScreen 信誉。

**不推荐的降本方式:** 不把私钥托管给不可信第三方,不合租硬件令牌,不把代码签名能力转交给无法审计的渠道。

### 风险披露

未签名 Windows 安装器意味着用户首次运行可能需要在 SmartScreen 中选择更多信息后继续。项目通过公开 release、镜像直链、`SHA256SUMS`、Tauri updater 签名和透明文档降低篡改与误解风险;这不能替代 Authenticode,但可以让用户在证书预算到位前做独立核验。

### CI / 发布管线(Authenticode 路径)

| 阶段 | 行为 | 阻塞? |
|---|---|---|
| `ci.yml` Rust | 独立跑 `codex-mac-engine` / `codex-win-engine` 测试 | 是(required) |
| `win-installer-check.yml` | 构建 x64 NSIS → Authenticode 探测 → 安装/启动/升级/卸载冒烟 | 否(路径变更时跑) |
| `release.yml` Windows | PE 架构诊断 → 可选 Authenticode 签名 → Authenticode 校验 → Tauri updater `.sig` → 收集**最终**工件 | updater `.sig` 与工件齐全为阻塞;Authenticode 默认非阻塞 |
| ARM64 | 交叉构建 + PE machine=`0xAA64` 诊断;**不是**实机运行验证 | 交叉构建失败阻塞;运行验证见下 |

**启用 Authenticode(证书就绪后):**

1. 在 `release` environment 配置 secrets:
   - `WINDOWS_CERTIFICATE` — base64 编码的 OV/EV `.pfx`
   - `WINDOWS_CERTIFICATE_PASSWORD` — pfx 密码
2. (可选) repo variable `WINDOWS_TIMESTAMP_URL` 覆盖默认 RFC3161 时间戳。
3. 确认 `release.yml` 的 *Authenticode-sign Windows installer* 步骤对 `-setup.exe` 签出 `Valid`。
4. 将 repo variable `AUTHENTICODE_REQUIRED=true`,使 `verify-windows-authenticode.ps1` 在 `required` 模式下失败即阻断发布。
5. 中期目标:在 `tauri build` 前导入证书并配置 `bundle.windows.certificateThumbprint`,让主程序 + uninstaller + installer 在打包期一并签名(比只签外层 setup 更完整)。

脚本:

- [`scripts/sign-windows-authenticode.ps1`](../scripts/sign-windows-authenticode.ps1) — 有证书则签,无证书则跳过(exit 0)。
- [`scripts/verify-windows-authenticode.ps1`](../scripts/verify-windows-authenticode.ps1) — `optional` / `required`。
- [`scripts/windows-packaged-smoke.ps1`](../scripts/windows-packaged-smoke.ps1) — x64 生命周期冒烟。
- [`scripts/windows-pe-arch.ps1`](../scripts/windows-pe-arch.ps1) — 读取 PE machine type。

失败日志阶段标签:`[build]` / `[sign]` / `[sign-verify]` / `[install]` / `[launch]` / `[upgrade]` / `[uninstall]`。

### ARM64 运行验证策略

- GitHub-hosted `windows-latest` 是 **x64**。`aarch64-pc-windows-msvc` 目标是交叉编译,产物经 `windows-pe-arch.ps1` 确认 machine=`0xAA64`。
- **交叉构建成功 ≠ 运行验证。** 完整 install/launch/upgrade/uninstall 冒烟只在 x64 runner 上对 x64 安装包执行。
- ARM64 实机或可信虚拟化验收清单(人工 / 自备 runner):
  1. 安装 `CodexAppManager_arm64-setup.exe`(被动 `/P` 或 UI)。
  2. 确认 `%LOCALAPPDATA%\Codex App Manager\codex-app-manager.exe` 存在且 PE 为 ARM64。
  3. 首次启动管理器 UI,无崩溃。
  4. 再跑一遍安装器 `/P /UPDATE` 升级路径。
  5. 卸载后主程序消失。
  6. 若已配置 Authenticode,确认 installer / 主程序 / uninstaller 的 `Get-AuthenticodeSignature` 为 `Valid`。

## English

### Current status

- macOS builds are Developer ID signed and Apple notarized.
- The Windows installers `CodexAppManager_x64-setup.exe` / `CodexAppManager_arm64-setup.exe` are not Authenticode-signed yet.
- Windows in-app update artifacts carry the Tauri updater signature, which verifies the downloaded bytes.
- SmartScreen may warn when users manually run the Windows installer for the first time; that is the known distribution risk, not an updater-signature failure.
- CI already runs x64 packaged lifecycle smoke (`install → launch → upgrade → uninstall`) and Authenticode probes; signing is skipped and verification is non-blocking until a certificate is configured.

### Three separate concepts

**Tauri updater signature:** The `signature` field in `latest.json` signs the installer bytes. It protects in-app self-update downloads from tampering across mirrors and network hops. It is not Windows publisher trust and does not remove SmartScreen warnings. Implementation: `npx tauri signer sign` + `TAURI_SIGNING_PRIVATE_KEY`.

**Authenticode code signing:** This is the Windows code-signing system for PE files and installers. With an OV or EV certificate, the installer can show a publisher identity and build operating-system trust. This project does not currently Authenticode-sign the Windows installer. The prepared CI path is documented below.

**SmartScreen reputation:** Microsoft Defender SmartScreen combines signing identity, download volume, historical reputation, and risk signals. EV certificates usually establish reputation faster; OV certificates and unsigned distribution still need reputation to build, and unsigned installers are more likely to warn on first run.

### How to verify downloads

1. Download the installer from [GitHub Releases](https://github.com/Wangnov/Codex-App-Manager/releases/latest) or the agentsmirror mirror.
2. Download `SHA256SUMS` from Assets on the same release.
3. Compute the local hash and compare it with the matching filename in `SHA256SUMS`.

Windows PowerShell:

```powershell
Get-FileHash .\CodexAppManager_x64-setup.exe -Algorithm SHA256
Get-FileHash .\CodexAppManager_arm64-setup.exe -Algorithm SHA256
```

macOS:

```bash
shasum -a 256 CodexAppManager_aarch64.dmg
shasum -a 256 CodexAppManager_x86_64.dmg
```

If the hash does not match, do not run the file. Download it again or open an
issue with the source URL and filename.

### Distribution channels and cost

**GitHub Releases + agentsmirror + SHA256SUMS:** This is the current primary channel. It is transparent, traceable, and independently verifiable, but the unsigned Windows installer can still trigger SmartScreen.

**winget:** `Wangnov.CodexAppManager` is available in microsoft/winget-pkgs, and this repository auto-submits new stable releases. winget accepts unsigned NSIS installers, but a new architecture or metadata change can still receive manual review.

**Microsoft Store / Partner Center:** This is a medium-term option. It requires a developer account, MSIX packaging, and Store review. It improves user trust but adds process and maintenance cost.

**OV / EV code-signing certificate:** A certificate is not a release blocker today. Re-evaluate when Windows download volume, enterprise demand, or support cost crosses a clear threshold. EV costs more but establishes reputation faster; OV costs less but still needs SmartScreen reputation to build.

**Cost shortcuts to avoid:** Do not custody private keys with untrusted third parties, share hardware tokens, or hand signing capability to channels that cannot be audited.

### Risk disclosure

An unsigned Windows installer means users may need to choose more information
and continue through SmartScreen on first run. The project reduces tampering and
confusion risk with public releases, mirror permalinks, `SHA256SUMS`, Tauri
updater signatures, and transparent documentation. Those mitigations do not
replace Authenticode, but they let users verify downloads independently until
certificate budget and demand justify Windows code signing.

### CI / release pipeline (Authenticode path)

| Stage | Behavior | Blocking? |
|---|---|---|
| `ci.yml` Rust | Standalone `codex-mac-engine` / `codex-win-engine` tests | Yes (required) |
| `win-installer-check.yml` | Build x64 NSIS → Authenticode probe → install/launch/upgrade/uninstall smoke | No (path-filtered) |
| `release.yml` Windows | PE arch diagnostic → optional Authenticode sign → Authenticode verify → Tauri updater `.sig` → collect **final** artifacts | Updater `.sig` + artifact set block; Authenticode optional by default |
| ARM64 | Cross-build + PE machine=`0xAA64` diagnostic; **not** runtime verification | Cross-build failure blocks; runtime verification below |

**Turning on Authenticode (when a cert is available):**

1. Add secrets on the `release` environment:
   - `WINDOWS_CERTIFICATE` — base64-encoded OV/EV `.pfx`
   - `WINDOWS_CERTIFICATE_PASSWORD` — pfx password
2. Optionally set repo variable `WINDOWS_TIMESTAMP_URL` for the RFC3161 timestamp server.
3. Confirm the *Authenticode-sign Windows installer* step produces `Valid` on `-setup.exe`.
4. Set repo variable `AUTHENTICODE_REQUIRED=true` so `verify-windows-authenticode.ps1` runs in `required` mode and fails the release if unsigned.
5. Medium-term: import the cert before `tauri build` and set `bundle.windows.certificateThumbprint` so the main binary, uninstaller, and installer are all signed during packaging (more complete than outer-setup-only signing).

Scripts:

- [`scripts/sign-windows-authenticode.ps1`](../scripts/sign-windows-authenticode.ps1) — signs when a cert is present; skips with exit 0 otherwise.
- [`scripts/verify-windows-authenticode.ps1`](../scripts/verify-windows-authenticode.ps1) — `optional` / `required`.
- [`scripts/windows-packaged-smoke.ps1`](../scripts/windows-packaged-smoke.ps1) — x64 lifecycle smoke.
- [`scripts/windows-pe-arch.ps1`](../scripts/windows-pe-arch.ps1) — PE machine type probe.

Failure log stage tags: `[build]` / `[sign]` / `[sign-verify]` / `[install]` / `[launch]` / `[upgrade]` / `[uninstall]`.

### ARM64 runtime verification strategy

- GitHub-hosted `windows-latest` is **x64**. The `aarch64-pc-windows-msvc` target is cross-compiled; `windows-pe-arch.ps1` asserts machine=`0xAA64`.
- **A successful cross-build is not runtime verification.** Full install/launch/upgrade/uninstall smoke runs only for the x64 installer on x64 runners.
- ARM64 bare-metal or trusted virtualization checklist (manual / self-hosted runner):
  1. Install `CodexAppManager_arm64-setup.exe` (passive `/P` or UI).
  2. Confirm `%LOCALAPPDATA%\Codex App Manager\codex-app-manager.exe` exists and is an ARM64 PE.
  3. First-launch the manager UI without crash.
  4. Re-run the installer with `/P /UPDATE` (upgrade path).
  5. Uninstall and confirm the main binary is gone.
  6. If Authenticode is configured, confirm installer / main / uninstaller `Get-AuthenticodeSignature` is `Valid`.
