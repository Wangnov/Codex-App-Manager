# Privacy policy · 隐私政策

Effective date / 生效日期：2026-07-11

This policy applies to Codex App Manager, its public website, and the download
router maintained in this repository. It does not replace the privacy policies
of GitHub, Cloudflare, IHEP, OpenAI, Microsoft, or a custom endpoint/proxy chosen
by the user.

本政策适用于 Codex App Manager、本项目官网以及本仓库维护的下载路由。GitHub、
Cloudflare、IHEP、OpenAI、Microsoft，以及用户自行选择的自定义源或代理，分别适用其
自己的隐私政策，本政策不能替代这些第三方政策。

## 我们不收集什么 · What we do not collect

- 应用没有账户系统，不要求姓名、邮箱、电话或付款信息。
- 应用不包含遥测、广告、崩溃上报或行为分析 SDK，也不会为画像目的上传使用行为。
- 项目不会主动读取或上传 `~/.codex` 中的对话、凭据、工作区文件或其他用户内容。
- 本地日志和设置不会自动发送给维护者。只有用户主动附在 Issue 或支持请求中时，维护者
  才会收到用户选择提交的内容。

- The app has no account system and does not request a name, email address,
  phone number, or payment information.
- The app includes no telemetry, advertising, crash-reporting, or behavioral
  analytics SDK and does not upload usage activity for profiling.
- The project does not intentionally read or upload conversations, credentials,
  workspace files, or other user content from `~/.codex`.
- Local logs and settings are not automatically sent to maintainers. Maintainers
  receive only the material a user deliberately attaches to an issue or support
  request.

## 应用会发起的网络请求 · Network requests made by the app

网络请求用于更新检查、下载和校验，不用于遥测：

- **Manager 自身更新**：应用启动约 1.5 秒后检查一次；保持打开时约每 6 小时检查一次。
  当前版本没有关闭 Manager 自身检查的设置。发现新版本后由用户决定是否安装。
- **Codex 更新**：默认在 Manager 主界面启动时检查，并在应用保持打开时按设置周期检查。
  用户可分别关闭启动检查和定时检查，也可调整定时间隔。
- **安装、更新和校验**：用户发起操作时，应用会访问所选的镜像、GitHub、OpenAI 官方
  源或用户配置的自定义 HTTPS 源，以取得 manifest、appcast、checksum 和安装包。
- **代理**：请求可使用系统代理、直连或用户配置的代理。自定义代理能看到经其转发的
  目标 host 和正常连接元数据。

These requests provide update, download, and verification functionality, not
telemetry:

- **Manager self-update** — one check runs about 1.5 seconds after startup and
  another runs about every six hours while the app remains open. The current
  version has no setting to disable Manager self-update checks. The user decides
  whether to install an available update.
- **Codex updates** — by default, the Manager checks when its main screen starts
  and periodically while open. Users can independently disable startup and
  periodic checks and can change the periodic interval.
- **Install, update, and verification** — when the user starts an operation, the
  app contacts the selected mirror, GitHub, official OpenAI source, or a custom
  HTTPS source to obtain manifests, appcasts, checksums, and installers.
- **Proxy behavior** — requests can use the system proxy, direct mode, or a
  user-configured proxy. A custom proxy can observe destination hosts and
  ordinary connection metadata for traffic it handles.

## 服务与连接元数据 · Services and connection metadata

默认网络路径可能包括：

- `codexapp.agentsmirror.com`：由 Cloudflare Worker 提供；全球通常使用 Cloudflare R2，
  中国大陆请求可能按 `CF-IPCountry` 路由到 IHEP S3 的临时预签名地址。
- `github.com`：Manager 自更新后备源及公开 release 下载。
- `persistent.oaistatic.com` 等 OpenAI 官方 host：用户选择官方源时获取 Codex appcast 或
  工件。
- 用户配置的自定义 HTTPS 源或代理。

相关第三方隐私政策：

- [GitHub General Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement)
- [Cloudflare Privacy Policy](https://www.cloudflare.com/privacypolicy/)
- [OpenAI Privacy Policy](https://openai.com/policies/privacy-policy/)
- [Microsoft Privacy Statement](https://privacy.microsoft.com/en-us/privacystatement)
- IHEP S3 为中国大陆镜像节点。截至本政策日期，项目未找到该镜像端点单独公开的隐私政策；
  相关处理受 IHEP/CAS 的适用服务条款与法律义务约束。可从
  [IHEP 官网](https://english.ihep.cas.cn/) 查询或联系其机构。若发现适用的官方政策，
  本页会补充直接链接。

这些服务与普通 HTTPS 服务一样，可能处理来源 IP、请求时间、请求路径、User-Agent、
响应状态以及防滥用/安全日志。项目维护者无法代表第三方承诺其保留期限；请查阅所选服务
的政策。官网自身不嵌入广告或分析脚本，但托管和路由服务仍可能生成正常的访问日志。

Default paths may include `codexapp.agentsmirror.com` (Cloudflare Worker, R2,
and possible IHEP S3 routing for mainland China), `github.com` (fallback update
and public release downloads), official OpenAI hosts such as
`persistent.oaistatic.com` when selected, and a custom HTTPS source or proxy.
Like ordinary HTTPS services, these providers may process source IP, request
time and path, User-Agent, response status, and security/abuse logs. Project
maintainers cannot promise third-party retention periods. The project website
embeds no advertising or analytics script, but hosting and routing providers
may still produce ordinary access logs.

Third-party privacy policies:

- [GitHub General Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement)
- [Cloudflare Privacy Policy](https://www.cloudflare.com/privacypolicy/)
- [OpenAI Privacy Policy](https://openai.com/policies/privacy-policy/)
- [Microsoft Privacy Statement](https://privacy.microsoft.com/en-us/privacystatement)
- IHEP S3 hosts the mainland-China mirror node. As of this policy date, the
  project has not identified a standalone public privacy policy for that mirror
  endpoint. Applicable IHEP/CAS service terms and legal obligations govern its
  processing; consult the [IHEP website](https://english.ihep.cas.cn/) or the
  institution directly. This page will add a direct link if an applicable
  official policy is identified.

## 本地数据 · Data stored locally

应用在本机保存运行所需的设置、安装来源信息、更新状态、操作状态和诊断日志。诊断信息可
包含应用版本、操作系统、架构、更新源 host、安装状态和错误文本。它不应包含代码签名
私钥、密码、SignPath token，或下载 URL 的临时签名 query；发现这类泄漏应按安全问题
处理。卸载 Manager 不等同于删除 Codex 自己的数据；卸载界面会明确说明保留范围。

The app stores settings, installation provenance, update state, operation state,
and diagnostic logs locally. Diagnostics can include app version, operating
system, architecture, update-source host, install state, and error text. They
must not contain code-signing private keys, passwords, SignPath tokens, or
temporary signed URL queries; any such leak is a security issue. Uninstalling
the Manager is not the same as deleting Codex data, and the uninstall UI states
what is retained.

## 用户选择与反馈 · User choices and contact

用户可以关闭 Codex 的启动/定时检查、选择更新源和代理，并在提交 Issue 前查看和删改诊断
内容。隐私或安全问题请通过
[GitHub Issues](https://github.com/Wangnov/Codex-App-Manager/issues) 报告；请勿公开粘贴
token、密码、私钥、完整预签名 URL 或其他秘密。

Users can disable Codex startup/periodic checks, select an update source and
proxy, and review or redact diagnostics before filing an issue. Report privacy
or security concerns through
[GitHub Issues](https://github.com/Wangnov/Codex-App-Manager/issues), and never
post tokens, passwords, private keys, complete presigned URLs, or other secrets
publicly.

See also the [code signing policy](./code-signing-policy.md).
