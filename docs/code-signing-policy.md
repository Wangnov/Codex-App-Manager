# Code signing policy · 代码签名政策

Last updated / 最后更新：2026-09-29

## Current status · 当前状态

Codex App Manager submitted an application to SignPath Foundation on
2026-07-11. **That application is no longer being pursued** — it went
unanswered for too long, so the project switched to a paid cloud-HSM
Authenticode certificate instead. No certificate, secret, or repository
variable is configured today, and no artifact has been Authenticode-signed.

Codex App Manager 已于 2026-07-11 提交 SignPath Foundation 申请，**该申请已不再
推进**——长期未获处理，项目因此改为采用按年付费的云 HSM Authenticode 证书方案。
本仓库当前未配置任何证书、secret 或 repo variable，也没有任何工件完成过
Authenticode 签名。

The current Windows installers are **not Authenticode-signed**. Their Tauri
updater signatures authenticate update bytes, but they are not Windows
publisher identity and do not remove SmartScreen warnings. The release
workflow contains a provider-agnostic Authenticode signing entry point
(`scripts/sign-windows-authenticode.ps1`, wired as Tauri's
`bundle.windows.signCommand`) that is a no-op — installers stay unsigned and
the release is still published — until the repository variable
`WINDOWS_SIGNING_PROVIDER` is explicitly set to a real provider. Unsigned
verification remains non-blocking until the separate repository variable
`AUTHENTICODE_REQUIRED` is set to `true`.

当前 Windows 安装器**没有 Authenticode 签名**。Tauri updater 签名只验证更新工件
字节，不代表 Windows 发行者身份，也不能消除 SmartScreen 提示。发布流程中有一个
provider-agnostic 的 Authenticode 签名入口
（`scripts/sign-windows-authenticode.ps1`，挂载为 Tauri 的
`bundle.windows.signCommand`），在 repo variable `WINDOWS_SIGNING_PROVIDER`
被显式设为某个真实供应商之前，它什么都不做——安装器保持未签名，发布照常进行。
未签名校验在另一个 repo variable `AUTHENTICODE_REQUIRED` 被设为 `true` 之前
同样不会阻断发布。

Production signing will be enabled only by a separate reviewed change, after a
real certificate is purchased and the integration has been verified end to end
with real x64 and ARM64 artifacts (see
[Artifact and verification requirements](#artifact-and-verification-requirements--工件与验证要求)
below). Until then, project pages and release notes must describe Windows
artifacts as unsigned and must not imply any provider has approved or signed
this project.

生产签名只会通过独立、受审查的变更来启用，前提是先购买真实证书，并用真实的 x64 /
ARM64 工件端到端验证接入（见下方[工件与验证要求](#artifact-and-verification-requirements--工件与验证要求)）。在此之前，项目页面与 release note 必须明确
Windows 工件未签名，不得暗示任何供应商已批准或已为本项目签名。

### Provider selection · 供应商选择

Two providers satisfy this policy's controls (private key never leaves a
cloud HSM, CI only ever handles a certificate thumbprint, no business
registration required for an individual maintainer):

以下两个供应商满足本政策的控制要求（私钥全程留在云 HSM 中不离开、CI 只经手证书
指纹、个人维护者无需公司注册资质）：

| Provider · 供应商 | Role · 角色 | `WINDOWS_SIGNING_PROVIDER` |
| --- | --- | --- |
| SSL.com eSigner (Individual Validated cert + eSigner CKA) | Recommended · 推荐 | `esigner` |
| Certum Open Source / Individual Cloud Code Signing (SimplySign) | Fallback · 备选 | `certum` |

Both are OV-tier certificates. Microsoft removed EV's special SmartScreen
"instant trust" shortcut from its Trusted Root Program in 2024; EV and OV now
build SmartScreen reputation identically through accumulated clean downloads,
so this project does not plan to purchase an EV certificate for that reason.
Full provider research, cost comparison, and the eligibility rationale are
kept in the project's internal records and summarized in
[`Windows signing and verification`](./windows-signing.md#provider-plan).

两者都是 OV 级证书。Microsoft 已于 2024 年从其 Trusted Root Program 中移除了 EV
证书曾经拥有的 SmartScreen“瞬时信任”捷径；EV 与 OV 现在通过累计的、无异常的下载量
以完全相同的方式建立信誉，因此本项目不打算为此购买 EV 证书。完整的供应商调研、
成本对比与资质依据保存在项目内部记录中，摘要见
[《Windows signing and verification》](./windows-signing.md#provider-plan)。

A `local-pfx` mode (`WINDOWS_SIGNING_PROVIDER=local-pfx`, base64 PFX in
`WINDOWS_CERTIFICATE` / `WINDOWS_CERTIFICATE_PASSWORD`) also exists for local
and CI testing — including CI's throwaway self-signed-certificate proof — and
must never be configured on the `release` environment as a substitute for a
real provider.

另有一个 `local-pfx` 模式（`WINDOWS_SIGNING_PROVIDER=local-pfx`，base64 PFX 存放于
`WINDOWS_CERTIFICATE` / `WINDOWS_CERTIFICATE_PASSWORD`）仅用于本地与 CI 测试——
包括 CI 中一次性自签名证书的链路证明——绝不能被配置到 `release` environment 中
用作真实供应商的替代品。

## Scope · 适用范围

- This policy covers only Codex App Manager artifacts built from this public,
  MIT-licensed repository.
- Any signing certificate configured under this policy will be used only for
  executable files and installers owned and maintained by this project. It
  will never be used for personal files, private or commercial software, or
  another project.
- The official OpenAI Codex desktop application is not embedded in or
  repackaged by the Manager installer. The Manager downloads official Codex
  artifacts only after the user requests an install or update.
- Open-source dependencies may be bundled where their licenses permit, but
  third-party binaries must not be presented or separately signed as
  project-owned code.

- 本政策只适用于从本 MIT 开源仓库构建的 Codex App Manager 工件。
- 本政策下配置的任何签名证书只会用于本项目拥有并维护的可执行文件和安装器，不会用于
  个人文件、私有/商业软件或其他项目。
- Manager 安装器不会嵌入或重新打包 OpenAI 官方 Codex 桌面应用。只有在用户主动发起安装
  或更新后，Manager 才会下载官方 Codex 工件。
- 在许可证允许时可以打包开源依赖，但不得把第三方二进制冒充或单独签署为本项目自有代码。

## Roles and review · 角色与审查

Current public role assignments are:

| Role | Member | Responsibility |
| --- | --- | --- |
| Committer / author | [@Wangnov](https://github.com/Wangnov) | Maintains source, build configuration, and release preparation. |
| Reviewer | [@Wangnov](https://github.com/Wangnov) | Reviews external contributions and verifies maintainer-authored PR diffs and required checks before merge. |
| Signing approver | [@Wangnov](https://github.com/Wangnov) | Once a provider is configured, clicks the required-reviewer approval on the `release` GitHub Environment for each production signing run (see [Source and release controls](#source-and-release-controls--源码与发布控制)) — the CI provisioning step's own automated provider login (SSL.com / Certum, with a stored TOTP secret) is not itself a human approval and does not satisfy this policy on its own. |

当前项目是单维护者项目，因此同一名维护者承担多个角色。外部贡献必须经维护者审查；维护者
自己的变更也必须通过 pull request、必需 CI 和明确的 diff/review 收尾后才可 squash 合并。
如果未来的团队结构要求不同人员之间的职责分离，项目会在启用生产签名前公开增加成员
并更新本表。

All GitHub and signing-provider accounts (SSL.com, Certum, or any future
provider) used for source control, release, approval, or administration must
use multi-factor authentication. Access is granted with least privilege, and
role changes must remain auditable.

用于源码、发布、审批或管理的 GitHub 与签名供应商（SSL.com、Certum 或未来的其他
供应商）账户必须启用多因素认证。权限按最小权限原则配置，角色变化必须可审计。

## Source and release controls · 源码与发布控制

- `main` is protected by an active GitHub ruleset. Changes enter through pull
  requests and must pass Frontend plus Rust checks on macOS and Windows.
- Force pushes and branch deletion are prohibited. Repository administrators
  technically retain an emergency bypass; a bypassed change must not be used
  for a signing request until its diff and CI have been reviewed and recorded.
- Signing only happens from the trusted GitHub Actions build for this public
  repository (`release.yml`, on a reviewed tag), tied to a reviewed
  commit/tag/workflow run. No signing secret is ever exposed to a
  `pull_request`-triggered workflow (see `win-installer-check.yml`, which
  proves the signing plumbing with a throwaway self-signed certificate
  instead of real credentials).
- Every production signing request requires a separate manual approval. No
  automatic approval, bulk pre-approval, or reuse of an old approval is
  permitted. This is enforced with a **required-reviewers protection rule on
  the `release` GitHub Environment** (Settings → Environments → `release` →
  Required reviewers): the `build` job that runs `tauri build` and signs all
  platforms already runs under `environment: release`, so once that rule is
  configured, GitHub itself pauses the job until the named reviewer approves
  that specific run — this is the actual per-request human gate, not the
  provider account login. The cloud-HSM provisioning step's own login to
  SSL.com/Certum (using a stored TOTP secret so it can run unattended) is
  automated and is **not** a substitute for this reviewer approval. A
  production provider (`esigner` or `certum`) must not be turned on for
  `WINDOWS_SIGNING_PROVIDER` until the required-reviewers rule is configured
  and verified to actually pause a run.

- `main` 由启用中的 GitHub ruleset 保护。所有变更通过 pull request 进入，并必须通过
  Frontend、macOS Rust 与 Windows Rust 检查。
- 禁止 force push 和删除分支。仓库管理员技术上仍有紧急 bypass；通过 bypass 进入的变更
  在 diff 与 CI 被重新审查并留下记录前，不得用于签名请求。
- 签名只能来自本公开仓库受信任的 GitHub Actions 构建（`release.yml`，由经过评审的
  tag 触发），绑定到经过评审的 commit / tag / workflow run。任何签名 secret 都不会
  暴露给 `pull_request` 触发的 workflow（见 `win-installer-check.yml`，它用一次性
  自签名证书证明签名链路，而不使用真实凭据）。
- 每一次生产签名请求都必须单独人工批准，不允许自动审批、批量预批准或复用旧审批。这
  通过在 `release` 这个 GitHub Environment 上配置**必需审批人（Required reviewers）**
  保护规则来落实（仓库 Settings → Environments → `release` → Required reviewers）：
  运行 `tauri build` 并为各平台签名的 `build` job 本身已经声明了
  `environment: release`，所以一旦配置了该规则，GitHub 会在这次具体运行被指定审批人
  批准之前自动暂停该 job——这才是真正的逐次人工关卡，而不是供应商账号登录。云 HSM
  预配步骤对 SSL.com / Certum 的登录本身是自动化的（依赖存储的 TOTP secret 才能无人
  值守运行），**不能**替代这道人工审批。在这条 required-reviewers 规则配置并验证能
  真正暂停某次运行之前，不得把 `WINDOWS_SIGNING_PROVIDER` 设为生产供应商
  （`esigner` 或 `certum`）。

## Artifact and verification requirements · 工件与验证要求

Before production signing can be enabled (`AUTHENTICODE_REQUIRED=true`), a
reviewed integration must prove all of the following with real x64 and ARM64
artifacts:

1. The signing request is bound to the expected repository, commit, release
   ref, workflow run, version, and artifact digest.
2. Artifact configuration enforces the Codex App Manager product name and a
   consistent product/file version.
3. The intended project-owned PE layers are covered: the installed main
   executable, uninstaller, and final installer — all three, signed inline
   during `tauri build` via `bundle.windows.signCommand`. Third-party
   binaries are not signed as project-owned files.
4. Every required PE reports a `Valid` Authenticode signature
   (`Get-AuthenticodeSignature` `Status -eq "Valid"`) from the expected
   publisher and carries a valid RFC3161 timestamp
   (`TimeStamperCertificate` present) — enforced by
   `scripts/verify-windows-authenticode.ps1` in `required` mode. "Expected
   publisher" is checked by exact certificate identity, not just validity:
   `release.yml` passes the specific thumbprint the `esigner`/`certum`
   provisioning step just loaded (`WINDOWS_SIGNING_THUMBPRINT`) as
   `-ExpectedThumbprint`, so a signature that is `Valid` but from a
   *different*, unrelated trusted certificate still fails the check — a
   plain `Status -eq "Valid"` check alone would not catch that. For x64 this
   includes the uninstaller, verified in `release.yml` by installing the
   real, just-signed release artifact (the uninstaller only exists once
   installed) and checking it against the same expected thumbprint.
   `windows-latest` is x64-only, so a cross-built ARM64 installer cannot be
   installed/run in CI; its uninstaller must be checked manually before each
   release using the checklist in
   [`Windows signing and verification`](./windows-signing.md#arm64-runtime-verification-strategy)
   until a native or trusted-virtualization ARM64 runner is available.
5. The Tauri updater signature is generated only after Authenticode signing so
   it authenticates the final published bytes.
6. Files uploaded to GitHub Releases and mirrors are byte-identical to the
   verified signed artifacts.
7. Any signing, timestamp, malware-scan, or post-signature verification
   failure blocks publication; there is no unsigned fallback once
   `AUTHENTICODE_REQUIRED=true` is set.

生产签名启用前（`AUTHENTICODE_REQUIRED=true`），独立受审查的接入必须用真实 x64 与
ARM64 工件证明以上全部条件。任何签名、时间戳、恶意软件扫描或签后校验失败都必须
阻断发布，一旦 `AUTHENTICODE_REQUIRED=true` 生效，不得回退为未签名发布。

Operational details, the provider comparison, and the current CI proof using a
throwaway self-signed certificate are documented in
[`Windows signing and verification`](./windows-signing.md). Network behavior
and user data handling are documented in the [privacy policy](./privacy.md).

## References · 参考

- [SSL.com eSigner CI/CD integration](https://www.ssl.com/how-to/cloud-code-signing-integration-with-github-actions/)
- [Certum SimplySign / cloud code signing](https://support.certum.eu/en/code-signing-required-documents/)
- [Privacy policy](./privacy.md)
- [Windows signing and verification](./windows-signing.md)
