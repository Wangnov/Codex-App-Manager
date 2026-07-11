# Code signing policy · 代码签名政策

Last updated / 最后更新：2026-07-11

## 当前状态 · Current status

Codex App Manager 正在申请 SignPath Foundation，并评估从 GitHub Actions trusted build
接入 Windows Authenticode 的可行方案。**申请尚未获批，证书尚未签发，SignPath 签名流水线
尚未启用；当前 Windows 安装器仍未带 Authenticode 发行者签名。** 在下列构建、审批和验证
条件全部满足前，tag 驱动的 Windows 正式发布会 fail closed，不会悄悄回退为 unsigned 发布。

Codex App Manager is applying to SignPath Foundation and evaluating a GitHub
Actions trusted-build integration for Windows Authenticode. **The application
has not yet been approved, no certificate has been issued, and no SignPath
signing pipeline is active. Current Windows installers remain unsigned.** The
tag-driven Windows release path fails closed until all build, approval, and
verification requirements below are met; it must never silently fall back to
publishing unsigned artifacts.

If the application is approved and the integration is activated, the required
provider attribution will be:

> Free code signing provided by [SignPath.io](https://signpath.io/), certificate by [SignPath Foundation](https://signpath.org/)

This sentence records the intended attribution and does not claim that the
current downloads are signed by SignPath.

## 适用范围 · Scope

- 本政策只适用于本仓库以 MIT 许可证发布、由本项目构建的 Codex App Manager 工件。
- Foundation 证书只会签署项目自身拥有和维护的可执行文件或安装器，不会用于其他项目、
  私有软件、商业软件或个人文件。
- OpenAI Codex 桌面应用不会被嵌入或重新打包进 Manager 安装器；Manager 运行后按用户选择
  下载官方 Codex 工件。
- 第三方依赖（包括 NSIS/Tauri 随附插件）可以作为开源依赖被打包，但不得被当作本项目
  自有二进制直接使用 Foundation 证书签名。接入方案必须证明这一边界。

- This policy covers only Codex App Manager artifacts built from this MIT-
  licensed public repository.
- A Foundation certificate will be used only for executables or installers
  owned and maintained by this project. It will not be used for other projects,
  private or commercial software, or personal files.
- The OpenAI Codex desktop app is not embedded or repackaged in the Manager
  installer. The Manager downloads official Codex artifacts after launch when
  the user requests an operation.
- Third-party dependencies, including plugins supplied with NSIS or Tauri, may
  be bundled as open-source dependencies but must not be directly signed as
  project-owned binaries with the Foundation certificate. The final integration
  must prove this boundary.

## 角色与人工审批 · Roles and manual approval

签名发布采用职责明确、逐次人工批准的流程：

1. **Author**：常规源码、依赖或发布配置变更必须通过 pull request 和 required checks
   进入 `main`。仓库管理员技术上拥有紧急 bypass；若确需使用，必须保留审计记录，并在
   发起任何签名请求前由 Reviewer 重新核对 bypass diff、commit 与 CI。
2. **Reviewer**：在合并前审查变更和必需 CI；不得用自动化结果替代代码审查。
3. **Release approver**：核对 tag、公开源码、构建来源、目标版本和待签工件后，在
   SignPath 中对**每一次**签名请求进行人工批准。不得自动批准、批量预批准或复用旧请求。
4. 角色成员及权限会在 SignPath 项目获批后按最小权限配置；GitHub 与 SignPath 账户必须
   启用双因素认证。角色或权限变化必须可审计。

当前团队成员（公开 GitHub 身份）：

| Role | Member | 说明 |
|---|---|---|
| Committer / Author | [@Wangnov](https://github.com/Wangnov) | 维护仓库、准备 maintainer PR 与 release tag；外部贡献者只通过 PR 提交，不自动取得此角色。 |
| Reviewer | [@Wangnov](https://github.com/Wangnov) | 审查外部 PR；对维护者自己的变更逐项复核 diff 与必需 CI。 |
| SignPath Approver | [@Wangnov](https://github.com/Wangnov) | 申请获批后，对每次签名请求执行单独的人工审批。 |

本项目目前是单维护者项目，因此同一人会兼任多个角色；这不会把签名批准自动化，也不会
省略每次请求的人工作业。如果 SignPath 的最终配置要求不同人员之间的职责分离，项目会在
启用生产签名前公开增补成员并更新本表。

The signing release process uses explicit responsibilities and per-request
human approval:

1. **Author** — normal source, dependency, and release-configuration changes
   must enter `main` through a pull request and required checks. Repository
   administrators technically retain an emergency bypass; if it is ever used,
   the action must remain auditable and the Reviewer must re-check the bypassed
   diff, commit, and CI before any signing request is submitted.
2. **Reviewer** — reviews the change and required CI before merge; automated
   checks do not replace code review.
3. **Release approver** — checks the tag, public source, build origin, target
   version, and candidate artifacts, then manually approves **each** SignPath
   signing request. Automatic approval, advance bulk approval, and reuse of an
   old request are prohibited.
4. Named role assignments and least-privilege access will be configured after
   the SignPath project is approved. GitHub and SignPath accounts must use
   two-factor authentication, and role changes must remain auditable.

Current team members (public GitHub identities):

| Role | Member | Responsibility |
|---|---|---|
| Committer / Author | [@Wangnov](https://github.com/Wangnov) | Maintains the repository and prepares maintainer PRs and release tags. External contributors submit PRs and do not automatically receive this role. |
| Reviewer | [@Wangnov](https://github.com/Wangnov) | Reviews external PRs and explicitly re-checks the diff and required CI for maintainer-authored changes. |
| SignPath Approver | [@Wangnov](https://github.com/Wangnov) | After approval, manually approves each individual signing request. |

This is currently a single-maintainer project, so one person holds multiple
roles. That does not automate signing approval or remove the manual action for
each request. If the final SignPath configuration requires separation between
different people, the project will add members publicly and update this table
before production signing is enabled.

## Trusted build 与发布门 · Trusted build and release gates

未来的 SignPath 接入必须使用 SignPath 验证过的 GitHub Actions trusted build/origin，且只
接受来自本公开仓库受保护 tag 的工件。正式启用前必须通过一次可复现的试签与审查，证明：

- 签名请求绑定准确的仓库、commit、tag、版本和 GitHub Actions run；
- 待签文件的产品名和版本资源与 tag 一致；
- x64 与 ARM64 的最终安装器、安装后的主程序和卸载器均满足设计的 Authenticode 边界，
  且没有把 Foundation 证书直接用于第三方插件；
- 每个要求签名的 PE 都显示预期的 SignPath Foundation 发行者并带有效时间戳；
- Tauri updater `.sig` 在 Authenticode 之后覆盖最终发布字节；它与 Authenticode 是两套
  独立的信任机制；
- 最终上传到 GitHub Release 和镜像的文件与已验证文件逐字节一致。

Until that review is complete, [`release.yml`](../.github/workflows/release.yml)
intentionally blocks Windows tag jobs before building. Removing the blocker
alone cannot make a release valid: unsigned artifacts must still fail the
required post-build verification gates. See
[`Windows signing and verification`](./windows-signing.md) for the operational
status and unresolved integration boundary.

## 隐私与网络行为 · Privacy and network behavior

完整、可稳定链接的隐私政策见 [`docs/privacy.md`](./privacy.md)。以下为与代码签名申请
直接相关的网络行为摘要。

See the stable, standalone [`privacy policy`](./privacy.md) for the complete
disclosure. The following is the network-behavior summary relevant to the code
signing application.

Codex App Manager 不提供账户系统，也不包含遥测、广告或行为分析 SDK，不会主动上传用户
身份、文件内容或使用画像。应用仍会为完成其公开功能发起网络请求：

- Manager 启动后以及保持打开期间会检查自身更新；发现更新后由用户决定是否安装。
- Codex 的更新检查可在设置中关闭启动检查和定时检查；安装或更新操作会按用户选择访问
  镜像、GitHub、OpenAI 官方源或用户配置的自定义源。
- 下载可使用系统代理、直连或用户配置的代理。所选服务会像任何 HTTPS 服务一样看到
  正常的连接元数据，例如来源 IP、时间、请求路径和 User-Agent；其日志受各服务自己的
  隐私政策和保留规则约束。
- 中国大陆的 `codexapp.agentsmirror.com` 请求可能由 Cloudflare 按地区路由到 IHEP S3；
  其他地区通常由 Cloudflare R2 提供。GitHub 是 Manager 自更新的后备源。
- 本地诊断日志可能记录版本、平台、更新源 host 和错误信息，但不应记录私钥、密码、
  SignPath token 或下载 URL 中的临时签名参数。用户只有在主动提交 Issue/诊断信息时才会
  把这些本地内容发送给项目维护者。

Codex App Manager has no account system and includes no telemetry, advertising,
or behavioral analytics SDK. It does not intentionally upload user identity,
file contents, or usage profiles. It still makes network requests to provide
its documented functionality:

- The Manager checks for its own updates after startup and periodically while
  open; the user chooses whether to install an available update.
- Startup and periodic update checks for Codex can be disabled in Settings.
  Install or update operations contact the selected mirror, GitHub, official
  OpenAI source, or a user-configured custom source.
- Downloads can use the system proxy, direct mode, or a custom proxy. Like any
  HTTPS service, the selected endpoint can observe ordinary connection metadata
  such as source IP, time, request path, and User-Agent; its own privacy and
  retention policies apply.
- Mainland-China requests to `codexapp.agentsmirror.com` may be routed by
  Cloudflare to IHEP S3; other regions are normally served from Cloudflare R2.
  GitHub is a fallback source for Manager self-updates.
- Local diagnostic logs may contain versions, platform, update-source host, and
  errors, but must not contain private keys, passwords, SignPath tokens, or
  temporary signed query strings. Local diagnostics reach maintainers only when
  a user deliberately includes them in an issue or support request.

## 参考 · References

- [SignPath Foundation terms](https://signpath.org/terms.html)
- [SignPath GitHub trusted build-system integration](https://docs.signpath.io/trusted-build-systems/github)
- [SignPath origin verification](https://docs.signpath.io/origin-verification/)
- [Privacy policy](./privacy.md)
- [Windows signing and verification](./windows-signing.md)
- [Release process](./release.md)
