# Code signing policy · 代码签名政策

Last updated / 最后更新：2026-07-14

## Current status · 当前状态

Codex App Manager submitted an application to SignPath Foundation on
2026-07-11. The application is still pending. No production SignPath
organization, project, artifact configuration, or signing policy is configured
for this repository, and no artifact has been signed through the Foundation
service.

Codex App Manager 已于 2026-07-11 提交 SignPath Foundation 申请，目前仍在审核。
本仓库尚未配置生产 SignPath organization、project、artifact configuration 或
signing policy，也没有任何工件通过 Foundation 服务完成签名。

The current Windows installers are **not Authenticode-signed**. Their Tauri
updater signatures authenticate update bytes, but they are not Windows
publisher identity and do not remove SmartScreen warnings. The current release
workflow still contains a legacy, optional PFX signing scaffold; it is not a
SignPath integration and unsigned verification remains non-blocking.

当前 Windows 安装器**没有 Authenticode 签名**。Tauri updater 签名只验证更新工件
字节，不代表 Windows 发行者身份，也不能消除 SmartScreen 提示。现有发布流程仍保留一条
可选的 PFX 签名占位路径；它不是 SignPath 接入，未签名校验目前也不会阻断发布。

SignPath support will be enabled only by a separate reviewed change after the
application is approved and the trusted-build integration has been verified
with real artifacts. Until then, project pages and release notes must describe
Windows artifacts as unsigned and must not imply Foundation approval.

只有在申请获批、并用真实工件验证 trusted-build 接入后，项目才会通过独立、受审查的变更
启用 SignPath。在此之前，项目页面与 release note 必须明确 Windows 工件未签名，不得暗示
Foundation 已批准本项目。

If the application is approved and production signing is activated, the
required attribution will be:

> Free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by [SignPath Foundation](https://signpath.org/)

This attribution records the intended provider relationship. It does **not**
claim that current downloads are signed.

## Scope · 适用范围

- This policy covers only Codex App Manager artifacts built from this public,
  MIT-licensed repository.
- The Foundation certificate will be used only for executable files and
  installers owned and maintained by this project. It will never be used for
  personal files, private or commercial software, or another project.
- The official OpenAI Codex desktop application is not embedded in or
  repackaged by the Manager installer. The Manager downloads official Codex
  artifacts only after the user requests an install or update.
- Open-source dependencies may be bundled where their licenses permit, but
  third-party binaries must not be presented or separately signed as
  project-owned code.

- 本政策只适用于从本 MIT 开源仓库构建的 Codex App Manager 工件。
- Foundation 证书只会用于本项目拥有并维护的可执行文件和安装器，不会用于个人文件、
  私有/商业软件或其他项目。
- Manager 安装器不会嵌入或重新打包 OpenAI 官方 Codex 桌面应用。只有在用户主动发起安装
  或更新后，Manager 才会下载官方 Codex 工件。
- 在许可证允许时可以打包开源依赖，但不得把第三方二进制冒充或单独签署为本项目自有代码。

## Roles and review · 角色与审查

Current public role assignments are:

| Role | Member | Responsibility |
| --- | --- | --- |
| Committer / author | [@Wangnov](https://github.com/Wangnov) | Maintains source, build configuration, and release preparation. |
| Reviewer | [@Wangnov](https://github.com/Wangnov) | Reviews external contributions and verifies maintainer-authored PR diffs and required checks before merge. |
| SignPath approver | [@Wangnov](https://github.com/Wangnov) | After approval, manually reviews and approves each individual signing request. |

当前项目是单维护者项目，因此同一名维护者承担多个角色。外部贡献必须经维护者审查；维护者
自己的变更也必须通过 pull request、必需 CI 和明确的 diff/review 收尾后才可 squash 合并。
如果 SignPath 的最终配置要求不同人员之间的职责分离，项目会在启用生产签名前公开增加成员
并更新本表。

All GitHub and SignPath accounts used for source control, release, approval, or
administration must use multi-factor authentication. Access is granted with
least privilege, and role changes must remain auditable.

用于源码、发布、审批或管理的 GitHub 与 SignPath 账户必须启用多因素认证。权限按最小权限
原则配置，角色变化必须可审计。

## Source and release controls · 源码与发布控制

- `main` is protected by an active GitHub ruleset. Changes enter through pull
  requests and must pass Frontend plus Rust checks on macOS and Windows.
- Force pushes and branch deletion are prohibited. Repository administrators
  technically retain an emergency bypass; a bypassed change must not be used
  for a signing request until its diff and CI have been reviewed and recorded.
- The future SignPath policy must accept artifacts only from the trusted GitHub
  Actions integration for this public repository, with origin verification
  bound to the reviewed source revision and approved release ref.
- Every production signing request requires a separate manual approval. No
  automatic approval, bulk pre-approval, or reuse of an old approval is
  permitted.

- `main` 由启用中的 GitHub ruleset 保护。所有变更通过 pull request 进入，并必须通过
  Frontend、macOS Rust 与 Windows Rust 检查。
- 禁止 force push 和删除分支。仓库管理员技术上仍有紧急 bypass；通过 bypass 进入的变更
  在 diff 与 CI 被重新审查并留下记录前，不得用于签名请求。
- 未来的 SignPath policy 只能接受来自本公开仓库、经 SignPath 信任的 GitHub Actions
  集成所生成的工件；origin verification 必须绑定已审查的源码 revision 与获准发布 ref。
- 每一次生产签名请求都必须单独人工批准，不允许自动审批、批量预批准或复用旧审批。

## Artifact and verification requirements · 工件与验证要求

Before production signing can be enabled, a reviewed integration must prove all
of the following with real x64 and ARM64 artifacts:

1. The signing request is bound to the expected repository, commit, release
   ref, workflow run, version, and artifact digest.
2. Artifact configuration enforces the Codex App Manager product name and a
   consistent product/file version.
3. The intended project-owned PE layers are covered: the installed main
   executable, uninstaller, and final installer. Third-party binaries are not
   signed as project-owned files.
4. Every required PE reports a valid Authenticode signature from the expected
   SignPath Foundation publisher and carries a valid timestamp.
5. The Tauri updater signature is generated only after Authenticode signing so
   it authenticates the final published bytes.
6. Files uploaded to GitHub Releases and mirrors are byte-identical to the
   verified signed artifacts.
7. Any signing, origin-verification, timestamp, malware-scan, or post-signature
   verification failure blocks publication; there is no unsigned fallback.

生产签名启用前，独立受审查的接入必须用真实 x64 与 ARM64 工件证明以上全部条件。任何签名、
来源验证、时间戳、恶意软件扫描或签后校验失败都必须阻断发布，不得回退为未签名发布。

Operational details and the current legacy scaffold are documented in
[`Windows signing and verification`](./windows-signing.md). Network behavior
and user data handling are documented in the [privacy policy](./privacy.md).

## References · 参考

- [SignPath Foundation conditions](https://signpath.org/terms.html)
- [SignPath GitHub trusted build systems](https://docs.signpath.io/trusted-build-systems/github)
- [SignPath origin verification](https://docs.signpath.io/origin-verification/)
- [Privacy policy](./privacy.md)
- [Windows signing and verification](./windows-signing.md)
