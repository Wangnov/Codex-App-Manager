# Windows signing and verification

This project currently ships a Windows NSIS installer without Authenticode code
signing. The installer is still published through GitHub Releases and the
agentsmirror download mirror, and every release includes `SHA256SUMS` so users
can verify the bytes they downloaded.

CI contains a provider-agnostic Authenticode signing entry point
(`scripts/sign-windows-authenticode.ps1`, wired as Tauri's
`bundle.windows.signCommand`), a verification script, and a CI proof that signs
throwaway binaries with a self-signed certificate. **SignPath Foundation is no
longer being pursued** — the free-signing application submitted on 2026-07-11
went unanswered for too long, so the project switched to a paid cloud-HSM
provider instead. See [Provider plan](#provider-plan) below and the
public [code-signing policy](code-signing-policy.md) and
[privacy policy](privacy.md).

## 中文

### 当前状态

- macOS 构建已经使用 Developer ID 签名并完成 Apple 公证。
- Windows 安装器 `CodexAppManager_x64-setup.exe` / `CodexAppManager_arm64-setup.exe` 当前没有 Authenticode 代码签名。
- Windows 应用内自更新包带有 Tauri updater 签名,用于校验下载字节没有被篡改。
- Windows 首次手动运行安装器时可能出现 SmartScreen 提示;这是预期风险,不是更新器签名失效。
- CI 已接入安装包冒烟(x64:`install → launch → upgrade → uninstall`)、Authenticode 探测,以及一个**不需要真实证书**的签名链路证明(用一次性自签名证书跑真实的签名脚本,断言签名者指纹匹配)。
- **SignPath Foundation 申请已不再推进。** 该免费签名申请于 2026-07-11 提交,长期未获处理;项目改为采用按年付费的云 HSM 证书提供方案(见下文)。证书购买前,发布始终保持未签名,不得声称已获批准或已签名。公开规则见[代码签名政策](code-signing-policy.md),数据边界见[隐私政策](privacy.md)。

### 三个概念

**Tauri updater 签名:** `latest.json` 里的 `signature` 对安装包字节签名。它保护应用内自更新下载,确保镜像或网络传输没有改包。它不参与 Windows 系统的发行者信任判断,也不能消除 SmartScreen 提示。实现:`npx tauri signer sign` + `TAURI_SIGNING_PRIVATE_KEY`。

**Authenticode 代码签名:** Windows 对 PE 文件和安装器使用的代码签名体系。由可信证书提供方签名后,安装器可以显示发行者身份,并逐步建立系统信任。本项目当前还没有给 Windows 安装器做 Authenticode 签名。

**SmartScreen 信誉:** Microsoft Defender SmartScreen 会结合签名身份、下载量、历史信誉和风险信号做拦截判断。**2024 年起,Microsoft 已从其 Trusted Root Program 中移除了 EV 证书曾经拥有的“瞬时信任”捷径** —— 截至本文档撰写时(2026),EV 与 OV 证书在 SmartScreen 信誉积累机制上完全等价,都需要靠累计的、无异常的下载量逐步建立信誉。因此不建议为“更快消除 SmartScreen 提示”而额外支付 EV 证书的费用。未签名分发的信誉积累速度最慢,首次运行也最容易被提示。

### 供应商方案

**当前状态:未配置任何供应商,发布保持未签名(默认行为,不会阻断发布)。**

签名入口统一为 [`scripts/sign-windows-authenticode.ps1`](../scripts/sign-windows-authenticode.ps1),它是 Tauri `bundle.windows.signCommand` 的挂载点(见 `src-tauri/tauri.conf.json`),在 `tauri build` 期间对主程序、NSIS 生成的 uninstaller、以及最终 installer 三层 PE 依次调用一次,由 repo variable `WINDOWS_SIGNING_PROVIDER` 决定具体走哪条签名路径:

| `WINDOWS_SIGNING_PROVIDER` | 说明 | 所需 secrets |
|---|---|---|
| (未设置 / `""` / `none`) | **默认。** 不签名,打印跳过信息并返回 0;与今天完全一致。 | 无 |
| `esigner` | **推荐方案:** SSL.com eSigner(Individual Validated 证书 + eSigner CKA 云 HSM)。 | `ESIGNER_USERNAME`、`ESIGNER_PASSWORD`、`ESIGNER_TOTP_SECRET` |
| `certum` | **备选方案:** Certum Open Source / Individual Cloud Code Signing(SimplySign 云 HSM),经社区维护的 `jay0lee/certum-cloud-code-sign` action 接入。 | `CERTUM_USERNAME`、`CERTUM_TOTP_SECRET` |
| `local-pfx` | **仅测试用**:导入一个 base64 PFX 到 `CurrentUser\My` 签名后立即删除。CI 的自签名证书链路证明就是用这个模式,不代表真实证书已配置。 | `WINDOWS_CERTIFICATE`、`WINDOWS_CERTIFICATE_PASSWORD`(均为 legacy 占位,production 场景不应使用) |

**迁移防护:** 在这个 provider 概念出现之前,`release.yml` 里的旧签名步骤只要 `WINDOWS_CERTIFICATE` secret 存在就会签名安装包。为避免有人已经配置了这个 secret,却因为忘记同步设置 `WINDOWS_SIGNING_PROVIDER` 而悄悄发布未签名版本,`sign-windows-authenticode.ps1` 现在会在“未设置 provider 但 `WINDOWS_CERTIFICATE` 已配置”这一种情况下直接报错终止,而不是静默跳过签名 —— 需要显式设置 `WINDOWS_SIGNING_PROVIDER=local-pfx`(或迁移到 `esigner`/`certum`)才能继续。仓库里此前从未配置过 `WINDOWS_CERTIFICATE`,因此这条防护不会改变任何人今天看到的行为。

选型依据(详见调研结论):SSL.com eSigner 是唯一同时满足“个人无需公司主体资质”“有官方维护的 GitHub Action(`SSLcom/esigner-codesign` / eSigner CKA)”“私钥全程留在云 HSM、CI 只经手证书指纹”三项要求的方案,列为推荐;Certum 的身份验证覆盖 180+ 国家且明确支持中文姓名音译,价格更低,但其 CI 自动化路径依赖社区脚本而非官方 action,稳定性稍弱,列为备选。两者都是 OV 级证书 —— 前面提到 EV 已不再有 SmartScreen 捷径,因此不建议为 EV 额外付费。

**开通新供应商的步骤(以 esigner 为例,certum 同理替换变量前缀):**

1. 购买 SSL.com IV 证书 + 一个 eSigner 签名额度套餐(可先用 30 天无限签名试用摸清每月实际签名次数)。
2. 在仓库 Settings → Environments → `release` 上配置**必需审批人(Required reviewers)**保护规则(如果还没配置的话)。运行 `tauri build` 并签名的 `build` job 已经声明了 `environment: release`,这条规则一旦配置,GitHub 会在每次该 job 运行时暂停,直到指定审批人点击批准 —— 这才是[代码签名政策](code-signing-policy.md)要求的逐次人工批准,供应商账号的自动登录不能替代它。
3. 在 GitHub 仓库的 `release` environment 中新增 secrets:`ESIGNER_USERNAME`、`ESIGNER_PASSWORD`、`ESIGNER_TOTP_SECRET`。
4. 设置 repo variable `WINDOWS_SIGNING_PROVIDER=esigner`。`release.yml` 会在 Windows job 的 `tauri build` 之前自动跑 “Provision eSigner CKA certificate” 步骤,把证书装进 runner 的证书库并导出指纹到 `WINDOWS_SIGNING_THUMBPRINT`;`tauri build` 期间 `signCommand` 会对三层 PE 依次调用 `scripts/sign-windows-authenticode.ps1` 完成签名。
5. **先在 `AUTHENTICODE_REQUIRED` 保持未设置(即 optional 模式)的情况下跑 1–2 次真实发布**,确认 x64 / arm64 上的三层 PE 均能验出 `Get-AuthenticodeSignature` 为 `Valid` 且带 RFC3161 时间戳(`TimeStamperCertificate` 非空)。
6. 确认无误后,把 repo variable `AUTHENTICODE_REQUIRED` 设为 `true`。此后 `verify-windows-authenticode.ps1` 在 `required` 模式下运行,x64 release job 还会多跑一步“Uninstaller Authenticode gate”(对刚签好名的 installer 做一次真实的安装/启动/升级/卸载,顺带验出 uninstaller 的签名);任何一层不是 `Valid` 或缺少时间戳都会阻断发布(不允许回退到未签名)。ARM64 在 x64 runner 上无法安装运行,uninstaller 签名需按下方 [ARM64 运行验证策略](#arm64-运行验证策略) 第 6 步人工核验。
7. 只有做到第 6 步之后,才能更新面向用户的文档(README、官网)声明 Windows 安装器已签名。

**仅 `certum` 需要的额外一步:** 设置 `WINDOWS_SIGNING_PROVIDER=certum` 之前,还需要设置 repo variable `CERTUM_ACTION_AUDITED=true` —— 否则 `release.yml` 会拒绝跑 Certum 的证书装载步骤。原因和“审计”具体指什么,见下方 [Certum action 审计要求](#certum-action-audit)。

**关键约束(见[代码签名政策](code-signing-policy.md)):**

- 签名只能来自受信任的 GitHub Actions 构建,绑定到经过评审的 commit / tag / workflow run。
- 每一次生产签名请求都需要独立的人工批准 —— 由 `release` GitHub Environment 上的 required-reviewers 规则落实(见上方第 2 步),不是云 HSM 供应商(SSL.com / Certum)的自动账号登录,也不做批量预批准。
- installer、主程序、uninstaller 三层 PE 都必须验出 `Valid` Authenticode 签名 + 时间戳(x64 由 `release.yml` 自动验证;ARM64 uninstaller 需人工核验)。
- 一旦 `AUTHENTICODE_REQUIRED=true` 生效,任何一层验证失败都必须阻断发布,不允许回退到未签名兜底。

### Certum action 审计要求 {#certum-action-audit}

`certum` 备选路径会用一个第三方组合式 GitHub Action —— 固定到单个 commit SHA 的 `jay0lee/certum-cloud-code-sign` —— 登录一个真实的 Certum SimplySign 账号(`CERTUM_USERNAME` + `CERTUM_TOTP_SECRET`,是完整账号凭据,不是限定权限的签名令牌)。和 `esigner`(SSL.com 官方维护、调用 SSL.com 自家 CLI 的 action)不同,这个 action 是社区自建的:审阅时它是一个刚创建不久、单一作者、没有历史积累的仓库,工作方式是通过自己的 `install`/`auth`/`verify` 脚本安装并 GUI 自动化操作 Certum SimplySign Desktop 客户端。SHA 固定能防止被锁定的那个 commit 内容事后被替换,但不代表项目里已经有人读过那个 commit 的脚本到底做了什么。

这直接触及本项目自己的[代码签名政策](code-signing-policy.md)——其中明确写着不得“把签名能力交给无法审计的渠道”。如果没有人先审查过代码,就把真实的 Certum 账号凭据交给一个第三方 action 里未经审查的自动化脚本、并让它在有凭据权限的 `release` environment 里跑,正是这条政策要防的情形。

因此 `release.yml` 给 Certum 的证书装载步骤加了第二道、独立的 repo variable 门槛:`CERTUM_ACTION_AUDITED=true`。只设置 `WINDOWS_SIGNING_PROVIDER=certum` 是不够的 —— “Require Certum action audit acknowledgment” 步骤会先失败,直到 `CERTUM_ACTION_AUDITED` 也被设置。这把“有没有人真的看过这段代码”从一段容易被忽略的文档文字,变成了一个必须单独、刻意完成的操作。设置它之前,维护者应当完成以下其中一项:

- 在固定的那个 commit(`jay0lee/certum-cloud-code-sign@a3324503499c49090869856eeeecfe95fb613d87`)上读完 `install.ps1`、`auth.mjs`、`verify.ps1`,确认它们只做 README 里声称的事;或者
- 把这些脚本的一份经过审查的副本/fork 直接 vendor 进本仓库,不再依赖上游 action,并把 `release.yml` 改成调用 vendor 进来的副本。

在完成其中一项之前,优先使用 `esigner`(官方维护、推荐的方案)—— `certum` 存在的意义是 `esigner` 不可用时的备选,而不是与其同等默认的选项。

### 用一次性自签名证书证明签名链路(无需真实证书)

`win-installer-check.yml` 的 `nsis` job 在每次改动打包/签名相关文件的 PR 上运行,用四步无需真实证书就证明整条签名链路:

1. **创建一次性自签名证书** —— 用 `New-SelfSignedCertificate` 现场生成一张一天有效期的自签名代码签名证书,导出为带随机密码的 base64 PFX。
2. **用这张证书签名地构建 NSIS installer** —— 把这个临时 PFX 以 `WINDOWS_SIGNING_PROVIDER=local-pfx` / `WINDOWS_CERTIFICATE` / `WINDOWS_CERTIFICATE_PASSWORD` 的形式喂给 `npm run tauri build`,让 Tauri 自己的 `bundle.windows.signCommand` 挂载点在一次真实构建中签名全部三层 PE(主程序、NSIS uninstaller、installer)——这验证的是 Tauri 集成本身,而不只是脱离 Tauri 单独调用签名脚本。
3. **断言构建产物携带这张证书** —— 用 `verify-windows-authenticode.ps1 -ExpectedThumbprint <临时证书指纹>` 断言构建出的主程序与 installer 的 `SignerCertificate.Thumbprint` 与临时证书完全一致。自签名证书永远不会被系统信任链接受,所以 `Status` 预期是 `UnknownError` / `NotTrusted` 而不是 `Valid`——`-ExpectedThumbprint` 才是让这个断言变得严格的关键:没有它,一个未签名文件在 `optional` 模式下会被静默放行。
4. **打包生命周期冒烟**,同样带上 `-ExpectedThumbprint` —— NSIS uninstaller 只有在真正安装后才会作为文件存在(由 NSIS 在安装期间写入),所以这是这个 job 里唯一能校验它签名的地方。这补上了"只在构建期证明"会留下的缺口:如果 Tauri 未来不再针对 uninstaller 单独调用 `signCommand`,只有这一步(而非上面构建产物那一步)能抓到。

这一整套证明的是"分发/签名逻辑本身工作正常,并且确实接入了真实的 `tauri build` + 安装路径、覆盖每一层 PE",不是"证书受信任"——后者是 `verify-windows-authenticode.ps1` 的 `required` 模式在真实供应商接入后的职责,同样通过 `-ExpectedThumbprint` 机制,只是这时指向真实供应商的证书指纹(见[生产验证要求](code-signing-policy.md#artifact-and-verification-requirements--工件与验证要求))。

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

**Microsoft Store / Partner Center:** 调研过 MSIX + Store 签名路径,但它解决不了当前问题:Store 签名的 MSIX 是只读沙箱安装,现有的 Tauri updater 自更新(原地替换二进制 + `latest.json`)在 MSIX 里跑不通,等于要换一套完全不同的分发/更新模型;且 Microsoft Store 在中国大陆的可用性历史上并不稳定。因此不作为 Authenticode 签名问题的解法,最多是未来的第二分发渠道。

**云 HSM 证书(esigner / certum):** 见上方[供应商方案](#供应商方案)。这是当前采用的方案。

**不推荐的降本方式:** 不把私钥托管给不可信第三方,不合租硬件令牌,不把代码签名能力转交给无法审计的渠道;不为了“更快建立 SmartScreen 信誉”而购买 EV 证书(2024 年后 EV 已无该捷径)。

### 风险披露

未签名 Windows 安装器意味着用户首次运行可能需要在 SmartScreen 中选择更多信息后继续。项目通过公开 release、镜像直链、`SHA256SUMS`、Tauri updater 签名和透明文档降低篡改与误解风险;这不能替代 Authenticode,但可以让用户在真实签名供应商接入完成前做独立核验。

### CI / 发布管线(Authenticode 路径)

| 阶段 | 行为 | 阻塞? |
|---|---|---|
| `ci.yml` Rust | 独立跑 `codex-mac-engine` / `codex-win-engine` 测试 | 是(required) |
| `win-installer-check.yml` | 构建 x64 NSIS → **自签名证书链路证明** → Authenticode 探测 → 安装/启动/升级/卸载冒烟 | 链路证明失败会阻塞该(非必需)workflow;Authenticode 探测本身非阻塞 |
| `release.yml` Windows | (若配置了 provider)证书预配 → `tauri build`(`signCommand` 内联签名三层 PE) → Authenticode 校验(主程序 + installer) → **x64 且 `AUTHENTICODE_REQUIRED=true` 时**:uninstaller 签名关卡(真实安装/启动/升级/卸载) → Tauri updater `.sig` → 收集**最终**工件 | updater `.sig` 与工件齐全为阻塞;Authenticode 默认非阻塞,`AUTHENTICODE_REQUIRED=true` 后阻塞(含 uninstaller 关卡) |
| ARM64 | 交叉构建 + PE machine=`0xAA64` 诊断;**不是**实机运行验证 | 交叉构建失败阻塞;运行验证见下 |

脚本:

- [`scripts/sign-windows-authenticode.ps1`](../scripts/sign-windows-authenticode.ps1) — provider-agnostic 签名入口,`WINDOWS_SIGNING_PROVIDER` 未设置时跳过(exit 0)。
- [`scripts/verify-windows-authenticode.ps1`](../scripts/verify-windows-authenticode.ps1) — `optional` / `required`;`required` 模式同时要求 `Valid` 状态与 RFC3161 时间戳。
- [`scripts/windows-packaged-smoke.ps1`](../scripts/windows-packaged-smoke.ps1) — x64 生命周期冒烟。
- [`scripts/windows-pe-arch.ps1`](../scripts/windows-pe-arch.ps1) — 读取 PE machine type。

失败日志阶段标签:`[build]` / `[sign]` / `[sign-proof]` / `[sign-verify]` / `[install]` / `[launch]` / `[upgrade]` / `[uninstall]`。

### ARM64 运行验证策略

- GitHub-hosted `windows-latest` 是 **x64**。`aarch64-pc-windows-msvc` 目标是交叉编译,产物经 `windows-pe-arch.ps1` 确认 machine=`0xAA64`。
- **交叉构建成功 ≠ 运行验证。** 完整 install/launch/upgrade/uninstall 冒烟只在 x64 runner 上对 x64 安装包执行。
- ARM64 实机或可信虚拟化验收清单(人工 / 自备 runner):
  1. 安装 `CodexAppManager_arm64-setup.exe`(被动 `/P` 或 UI)。
  2. 确认 `%LOCALAPPDATA%\Codex App Manager\codex-app-manager.exe` 存在且 PE 为 ARM64。
  3. 首次启动管理器 UI,无崩溃。
  4. 再跑一遍安装器 `/P /UPDATE` 升级路径。
  5. 卸载后主程序消失。
  6. 若已配置签名供应商,确认 installer / 主程序 / uninstaller 的 `Get-AuthenticodeSignature` 为 `Valid` 且带时间戳。

## English

### Current status

- macOS builds are Developer ID signed and Apple notarized.
- The Windows installers `CodexAppManager_x64-setup.exe` / `CodexAppManager_arm64-setup.exe` are not Authenticode-signed yet.
- Windows in-app update artifacts carry the Tauri updater signature, which verifies the downloaded bytes.
- SmartScreen may warn when users manually run the Windows installer for the first time; that is the known distribution risk, not an updater-signature failure.
- CI already runs x64 packaged lifecycle smoke (`install → launch → upgrade → uninstall`), Authenticode probes, and a **no-real-certificate-required** signing-plumbing proof (signs throwaway binaries with a self-signed certificate and asserts the signer thumbprint matches).
- **The SignPath Foundation application is no longer being pursued.** It was submitted on 2026-07-11 and went unanswered for too long, so the project switched to a paid cloud-HSM certificate provider instead (see [Provider plan](#provider-plan) below). Releases stay unsigned until a certificate is actually configured, and must never be described as approved or signed before that. See the public [code-signing policy](code-signing-policy.md) and [privacy policy](privacy.md).

### Three separate concepts

**Tauri updater signature:** The `signature` field in `latest.json` signs the installer bytes. It protects in-app self-update downloads from tampering across mirrors and network hops. It is not Windows publisher trust and does not remove SmartScreen warnings. Implementation: `npx tauri signer sign` + `TAURI_SIGNING_PRIVATE_KEY`.

**Authenticode code signing:** This is the Windows code-signing system for PE files and installers. A trusted certificate provider can let the installer show a publisher identity and build operating-system trust. This project does not currently Authenticode-sign the Windows installer.

**SmartScreen reputation:** Microsoft Defender SmartScreen combines signing identity, download volume, historical reputation, and risk signals. **Microsoft removed EV's special "instant trust" shortcut from its Trusted Root Program in 2024** — as of this writing (2026), EV and OV certificates build SmartScreen reputation identically, purely through accumulated, clean download volume over time. There is therefore no cost-justified reason to pay extra for an EV certificate just to reduce SmartScreen warnings faster. Unsigned distribution builds reputation slowest and is most likely to warn on first run.

### Provider plan

**Current status: no provider configured, releases stay unsigned (the default, non-blocking behavior).**

The single signing entry point is [`scripts/sign-windows-authenticode.ps1`](../scripts/sign-windows-authenticode.ps1), wired as Tauri's `bundle.windows.signCommand` (see `src-tauri/tauri.conf.json`). Tauri invokes it once per PE layer during `tauri build` — the main executable, the generated NSIS uninstaller, and the final installer — and the script dispatches on the repo variable `WINDOWS_SIGNING_PROVIDER`. `tauri build` changes its process working directory to `src-tauri/` before bundling, so `signCommand`'s `-File` path is `../scripts/sign-windows-authenticode.ps1`, relative to `src-tauri/`, not the repo root:

| `WINDOWS_SIGNING_PROVIDER` | Meaning | Required secrets |
|---|---|---|
| (unset / `""` / `none`) | **Default.** No signing; prints a skip message and returns 0 — exactly today's behavior. | none |
| `esigner` | **Recommended:** SSL.com eSigner (Individual Validated certificate + eSigner CKA cloud HSM). | `ESIGNER_USERNAME`, `ESIGNER_PASSWORD`, `ESIGNER_TOTP_SECRET` |
| `certum` | **Fallback:** Certum Open Source / Individual Cloud Code Signing (SimplySign cloud HSM), integrated via the community-maintained `jay0lee/certum-cloud-code-sign` action. | `CERTUM_USERNAME`, `CERTUM_TOTP_SECRET` |
| `local-pfx` | **Testing only:** imports a base64 PFX into `CurrentUser\My`, signs, and removes it immediately. This is the mode CI's self-signed-certificate proof uses; it never implies a real certificate is configured. | `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD` (legacy scaffold; not for production use) |

**Migration guard:** before this provider concept existed, `release.yml`'s old signing step signed the installer whenever the `WINDOWS_CERTIFICATE` secret alone was present. To make sure nobody who had already configured that secret silently ends up with an unsigned release just because `WINDOWS_SIGNING_PROVIDER` was never added, `sign-windows-authenticode.ps1` now fails loudly — instead of skipping silently — when `WINDOWS_CERTIFICATE` is set but no provider is configured; it requires explicitly setting `WINDOWS_SIGNING_PROVIDER=local-pfx` (or migrating to `esigner`/`certum`) to proceed. This repository has never had `WINDOWS_CERTIFICATE` configured on the `release` environment (confirmed via the GitHub API), so this guard does not change today's behavior for anyone.

Why this pairing (full research write-up kept internally): SSL.com eSigner is the only option researched with (a) confirmed individual, no-business-registration eligibility, (b) a vendor-maintained GitHub Action (`SSLcom/esigner-codesign` / eSigner CKA), and (c) a cloud-HSM design where the private key never leaves the provider — CI only ever handles a certificate thumbprint. Certum accepts ID documents from 180+ countries (with explicit support for non-Latin name transliteration) at a lower price, but its CI automation is community-built rather than vendor-published, so it is the fallback. Both are OV-tier certificates — since EV no longer has a SmartScreen shortcut (see above), paying for EV is not recommended.

**Steps to turn on a provider (using `esigner`; substitute the `CERTUM_*` variable names for `certum`):**

1. Purchase an SSL.com IV certificate plus an eSigner signing-tier subscription (use the 30-day unlimited-signing trial first to size real monthly signing volume).
2. Configure a **required-reviewers** protection rule on the `release` GitHub Environment (Settings → Environments → `release` → Required reviewers) if it isn't already set. The `build` job that runs `tauri build` and signs every platform already declares `environment: release`, so once this rule exists, GitHub pauses that job on every run until the named reviewer approves it — this is the per-request human approval [the code-signing policy](code-signing-policy.md) requires; the provider's own automated account login is not a substitute for it.
3. Add `ESIGNER_USERNAME`, `ESIGNER_PASSWORD`, and `ESIGNER_TOTP_SECRET` as secrets on the GitHub `release` environment.
4. Set the repo variable `WINDOWS_SIGNING_PROVIDER=esigner`. `release.yml`'s Windows job then runs a "Provision eSigner CKA certificate" step before `tauri build`, installing the certificate into the runner's store and exporting its thumbprint as `WINDOWS_SIGNING_THUMBPRINT`; the `signCommand` hook signs all three PE layers with it during `tauri build`.
5. **Run 1–2 real releases with `AUTHENTICODE_REQUIRED` left unset (optional mode) first**, and confirm all three PE layers on both x64 and arm64 verify `Get-AuthenticodeSignature` as `Valid` with an RFC3161 timestamp present (`TimeStamperCertificate` non-null).
6. Only once that is proven, set the repo variable `AUTHENTICODE_REQUIRED=true`. `verify-windows-authenticode.ps1` then runs in `required` mode, checking both `Status -eq "Valid"`/timestamp AND (via `-ExpectedThumbprint $env:WINDOWS_SIGNING_THUMBPRINT`) that the signature is specifically from the certificate this run's provisioning step just loaded, not merely from *some* trusted certificate. The x64 release job additionally runs an "Uninstaller Authenticode gate" step — a real install/launch/upgrade/uninstall pass against the just-signed installer, which also verifies the uninstaller's signature the same way (it only exists once installed). Any PE layer that is not `Valid`, lacks a timestamp, or was signed by the wrong certificate, blocks the release — there is no unsigned fallback once this is on. ARM64 cannot install/run on an x64 runner, so its uninstaller must be checked manually — see [ARM64 runtime verification strategy](#arm64-runtime-verification-strategy) step 6.
7. Only after step 6 is proven should user-facing docs (README, website) claim the Windows installers are signed.

**Additional step for `certum` only:** before setting `WINDOWS_SIGNING_PROVIDER=certum`, also set the repo variable `CERTUM_ACTION_AUDITED=true` — `release.yml` refuses to run the Certum provisioning step without it. See [Certum action audit](#certum-action-audit) below for why this exists and what "audited" means here.

**Hard constraints (see the [code-signing policy](code-signing-policy.md)):**

- Signing only happens from a trusted GitHub Actions build tied to a reviewed commit/tag/workflow run.
- Every production signing request requires a separate manual approval, enforced by the `release` GitHub Environment's required-reviewers rule (see step 2 above) — the cloud HSM provider's (SSL.com / Certum) own automated account login is not that manual gate on its own; no bulk pre-approval.
- All three PE layers (installer, main executable, uninstaller) must show a `Valid` Authenticode signature plus a timestamp — automatically verified for x64 by `release.yml`; ARM64's uninstaller is checked manually.
- Once `AUTHENTICODE_REQUIRED=true` is on, any verification failure must block the release — no unsigned fallback.

### Certum action audit {#certum-action-audit}

The `certum` fallback path authenticates to a real Certum SimplySign account (full account credentials — `CERTUM_USERNAME` + `CERTUM_TOTP_SECRET`, not a scoped signing token) by running a third-party composite GitHub Action, `jay0lee/certum-cloud-code-sign`, pinned to a single commit SHA. Unlike `esigner` (an SSL.com-maintained action calling SSL.com's own CLI), this action is community-built: at review time it was a very recently created, single-author repository with no prior track record, and it works by installing and GUI-automating the Certum SimplySign Desktop client through its own `install`/`auth`/`verify` scripts. SHA-pinning stops the pinned commit's *content* from changing after the fact, but it does not mean anyone on this project has read what that pinned commit's scripts actually do.

That directly matters here: this project's own [code-signing policy](code-signing-policy.md) says not to "hand signing capability to channels that cannot be audited." Handing real Certum account credentials to a third-party action's unreviewed automation scripts inside the credentialed `release` environment would do exactly that unless someone has actually reviewed the code first.

So `release.yml` gates the Certum provisioning step behind a second, explicit repo variable: `CERTUM_ACTION_AUDITED=true`. Setting `WINDOWS_SIGNING_PROVIDER=certum` alone is not enough — the "Require Certum action audit acknowledgment" step fails the job until `CERTUM_ACTION_AUDITED` is also set. This turns "did someone actually look at this" from an easy-to-miss doc paragraph into a required, separate, deliberate action. Before setting it, a maintainer should do one of:

- Read `install.ps1`, `auth.mjs`, and `verify.ps1` at the exact pinned commit (`jay0lee/certum-cloud-code-sign@a3324503499c49090869856eeeecfe95fb613d87`) and confirm they do only what the README claims; or
- Vendor a reviewed fork/copy of those scripts into this repository instead of depending on the upstream action at all, and update `release.yml` to call the vendored copy.

Until one of those happens, prefer `esigner` (the recommended, vendor-maintained provider) — `certum` exists as a fallback in case `esigner` becomes unavailable, not as an equally-default choice.

### Proving the signing plumbing without a real certificate

`win-installer-check.yml`'s `nsis` job runs on every PR that touches packaging/signing files, and proves the real signing path end to end with no real certificate:

1. **Create throwaway self-signed code-signing certificate** — a one-day-valid certificate via `New-SelfSignedCertificate`, exported as a base64 PFX with a random per-run password.
2. **Build NSIS installer (signed with throwaway certificate)** — the throwaway PFX is fed into `npm run tauri build` as `WINDOWS_SIGNING_PROVIDER=local-pfx` / `WINDOWS_CERTIFICATE` / `WINDOWS_CERTIFICATE_PASSWORD`, so Tauri's own `bundle.windows.signCommand` hook actually signs all three PE layers (main exe, NSIS uninstaller, installer) during a real build — this tests the actual Tauri integration, not just the sign script called in isolation.
3. **Assert build artifacts carry the throwaway certificate** — `verify-windows-authenticode.ps1 -ExpectedThumbprint <throwaway thumbprint>` asserts the built main exe and installer's `SignerCertificate.Thumbprint` exactly matches the throwaway certificate. A self-signed certificate never chains to a trusted root, so `Status` is expected to be `UnknownError`/`NotTrusted`, never `Valid` — `-ExpectedThumbprint` is what makes this assertion strict regardless of `Status`; without it, an unsigned file would otherwise soft-pass in `optional` mode.
4. **Packaged lifecycle smoke**, with the same `-ExpectedThumbprint` threaded through — the NSIS uninstaller only exists as a file once the package is actually installed (NSIS writes it during install), so this is the only place this job can check its signature at all. This closes the gap a build-time-only proof would have: if Tauri ever stopped invoking `signCommand` for the uninstaller specifically, this step (not just the main-exe/installer check above) would catch it.

Together these prove "the dispatch/signing logic works, and it is actually wired into the real `tauri build` + install path for every PE layer" — not trust; trust (a certificate chaining to a public root) is what `verify-windows-authenticode.ps1`'s `required` mode checks once a real provider is configured, via the same `-ExpectedThumbprint` mechanism pinned to the real provider's certificate (see the [production verification requirements](code-signing-policy.md#artifact-and-verification-requirements--工件与验证要求)).

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

**Microsoft Store / Partner Center:** Researched and set aside: a Store-signed MSIX installs read-only/sandboxed, which is incompatible with the current Tauri updater (in-place binary replacement + `latest.json`) — adopting it would mean replacing the whole distribution/update model, not just adding a signature. Microsoft Store availability in mainland China has also historically been unreliable. It is not a fix for Authenticode signing; at most a possible future second channel.

**Cloud-HSM certificate (esigner / certum):** See [Provider plan](#provider-plan) above. This is the adopted approach.

**Cost shortcuts to avoid:** Do not custody private keys with untrusted third parties, share hardware tokens, or hand signing capability to channels that cannot be audited; do not buy an EV certificate just to build SmartScreen reputation faster (EV lost that shortcut in 2024).

### Risk disclosure

An unsigned Windows installer means users may need to choose more information
and continue through SmartScreen on first run. The project reduces tampering and
confusion risk with public releases, mirror permalinks, `SHA256SUMS`, Tauri
updater signatures, and transparent documentation. Those mitigations do not
replace Authenticode, but they let users verify downloads independently until
a real signing provider is configured and proven.

### CI / release pipeline (Authenticode path)

| Stage | Behavior | Blocking? |
|---|---|---|
| `ci.yml` Rust | Standalone `codex-mac-engine` / `codex-win-engine` tests | Yes (required) |
| `win-installer-check.yml` | Build x64 NSIS → **self-signed signCommand proof** → Authenticode probe → install/launch/upgrade/uninstall smoke | The proof step blocks this (non-required) workflow on failure; the Authenticode probe itself stays non-blocking |
| `release.yml` Windows | (if a provider is configured) provision certificate → `tauri build` (`signCommand` signs all three PE layers inline) → Authenticode verify (main exe + installer) → **x64 and `AUTHENTICODE_REQUIRED=true` only:** uninstaller Authenticode gate (real install/launch/upgrade/uninstall) → Tauri updater `.sig` → collect **final** artifacts | Updater `.sig` + artifact set block; Authenticode is non-blocking by default, blocking (including the uninstaller gate) once `AUTHENTICODE_REQUIRED=true` |
| ARM64 | Cross-build + PE machine=`0xAA64` diagnostic; **not** runtime verification | Cross-build failure blocks; runtime verification below |

Scripts:

- [`scripts/sign-windows-authenticode.ps1`](../scripts/sign-windows-authenticode.ps1) — provider-agnostic signing entry point; a no-op (exit 0) while `WINDOWS_SIGNING_PROVIDER` is unset.
- [`scripts/verify-windows-authenticode.ps1`](../scripts/verify-windows-authenticode.ps1) — `optional` / `required`; `required` mode now also requires an RFC3161 timestamp.
- [`scripts/windows-packaged-smoke.ps1`](../scripts/windows-packaged-smoke.ps1) — x64 lifecycle smoke.
- [`scripts/windows-pe-arch.ps1`](../scripts/windows-pe-arch.ps1) — PE machine type probe.

Failure log stage tags: `[build]` / `[sign]` / `[sign-proof]` / `[sign-verify]` / `[install]` / `[launch]` / `[upgrade]` / `[uninstall]`.

### ARM64 runtime verification strategy

- GitHub-hosted `windows-latest` is **x64**. The `aarch64-pc-windows-msvc` target is cross-compiled; `windows-pe-arch.ps1` asserts machine=`0xAA64`.
- **A successful cross-build is not runtime verification.** Full install/launch/upgrade/uninstall smoke runs only for the x64 installer on x64 runners.
- ARM64 bare-metal or trusted virtualization checklist (manual / self-hosted runner):
  1. Install `CodexAppManager_arm64-setup.exe` (passive `/P` or UI).
  2. Confirm `%LOCALAPPDATA%\Codex App Manager\codex-app-manager.exe` exists and is an ARM64 PE.
  3. First-launch the manager UI without crash.
  4. Re-run the installer with `/P /UPDATE` (upgrade path).
  5. Uninstall and confirm the main binary is gone.
  6. If a signing provider is configured, confirm installer / main / uninstaller `Get-AuthenticodeSignature` is `Valid` with a timestamp.
