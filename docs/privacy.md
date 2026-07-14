# Privacy policy · 隐私政策

Effective date / 生效日期：2026-07-14

This policy applies to Codex App Manager, its public website, and the download
router maintained by this repository. Third-party services selected or contacted
by the user remain subject to their own privacy policies.

本政策适用于 Codex App Manager、本项目官网以及本仓库维护的下载路由。用户选择或应用
为完成公开功能而访问的第三方服务，仍分别适用其自身的隐私政策。

## What we do not collect · 我们不收集什么

- The app has no account system and does not ask for a name, email address,
  phone number, or payment information.
- The app and website contain no project-operated telemetry, advertising,
  crash-reporting, or behavioral-analytics service.
- The project does not intentionally read or upload conversations,
  credentials, workspace files, or other user content from `~/.codex`.
- Settings, logs, and diagnostics remain local unless the user deliberately
  copies or attaches them to an issue or support request.

- 应用没有账户系统，不要求姓名、邮箱、电话或付款信息。
- 应用与官网不包含由本项目运营的遥测、广告、崩溃上报或行为分析服务。
- 项目不会主动读取或上传 `~/.codex` 中的对话、凭据、工作区文件或其他用户内容。
- 设置、日志和诊断默认只保存在本机；只有用户主动复制或附加到 Issue/支持请求时才会离开
  本机。

## Network requests made by the app · 应用发起的网络请求

Network requests provide update, download, and verification features; they are
not used for telemetry:

- **Manager self-update:** the current version checks only when the user selects
  “Check for manager updates” on the About screen. Download, installation, and
  restart require a separate user confirmation.
- **Codex update checks:** by default, the Manager checks Codex on the Home
  screen at startup and periodically while the Manager remains open. Users can
  independently disable startup and periodic checks and adjust the interval.
- **Install, update, and verification:** a user-initiated operation contacts the
  selected mirror, official OpenAI source, GitHub fallback, or a custom HTTPS
  source to obtain manifests, appcasts, checksums, and installers.
- **Proxy behavior:** requests may use the system mode, direct mode, or a proxy
  configured by the user. A custom proxy can observe destination hosts and
  ordinary connection metadata for traffic it handles.
- **External links:** repository, feedback, and policy links open only after a
  user action.

网络请求只用于更新、下载与校验，不用于遥测：

- **Manager 自身更新：**当前版本只有在用户进入“关于”并点击“检查管理器更新”时才会检查；
  下载、安装与重启还需要用户再次确认。
- **Codex 更新检查：**默认在主界面启动时检查，并在 Manager 保持打开时按设置周期检查。
  用户可以分别关闭启动检查和定时检查，也可调整间隔。
- **安装、更新与校验：**用户主动发起操作后，应用会访问所选镜像、OpenAI 官方源、GitHub
  后备源或自定义 HTTPS 源，以取得 manifest、appcast、checksum 与安装包。
- **代理行为：**请求可使用系统模式、直连模式或用户配置的代理。自定义代理能够看到它所
  转发流量的目标 host 和正常连接元数据。
- **外部链接：**仓库、反馈与政策链接只会在用户主动点击后打开。

## Services and connection metadata · 服务与连接元数据

Default network paths may include:

- `codexapp.agentsmirror.com` for Manager and Codex manifests, downloads, and
  update metadata. Cloudflare normally serves or routes these requests; mainland
  China requests may be redirected to an IHEP S3 mirror.
- `github.com` and `githubusercontent.com` for public repository pages and
  Manager update fallback artifacts.
- OpenAI-operated hosts such as `persistent.oaistatic.com` when the official
  Codex source is selected.
- `go.microsoft.com` when the Windows installer needs to download Microsoft's
  WebView2 bootstrapper before Codex App Manager can run. Systems that already
  have a suitable WebView2 runtime do not need this download.
- A custom HTTPS source or proxy explicitly configured by the user.

这些服务与普通 HTTPS 服务一样，可能处理来源 IP、请求时间与路径、User-Agent、响应状态
以及防滥用/安全日志。项目维护者无法代表第三方承诺其保留期限。相关第三方政策包括：

- [GitHub General Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement)
- [Cloudflare Privacy Policy](https://www.cloudflare.com/privacypolicy/)
- [OpenAI Privacy Policy](https://openai.com/policies/privacy-policy/)
- [Microsoft Privacy Statement](https://privacy.microsoft.com/privacystatement)

Windows 首次安装时，如果系统没有可用的 WebView2 runtime，NSIS 安装器会在 Manager
启动前通过 `go.microsoft.com` 下载并运行 Microsoft WebView2 bootstrapper；已有合适
runtime 的系统不需要这次下载。该连接适用 Microsoft 的隐私政策。

IHEP S3 hosts the mainland-China mirror node. As of this policy date, the
project has not identified a standalone public privacy policy for that mirror
endpoint. Applicable institutional terms and legal obligations govern its
processing; see the [IHEP website](https://english.ihep.cas.cn/). This document
will add a direct policy link if one becomes available.

## Website storage · 官网本地存储

The website stores only the visitor's language choice in browser local storage
under `cam-site-lang`. It embeds no advertising or analytics script. Cloudflare,
as the hosting and routing provider, may still generate ordinary access and
security logs under its own policy.

官网只在浏览器 local storage 的 `cam-site-lang` 中保存语言选择，不嵌入广告或分析脚本。
作为托管和路由服务商，Cloudflare 仍可能按其政策生成正常的访问与安全日志。

## Local data and diagnostics · 本地数据与诊断

The app stores settings, installation provenance, update state, operation state,
and diagnostic logs on the local machine. A copied diagnostic report can include
the app version, operating system and architecture, update-source host, install
status, local log paths, error text, and—when an upstream tool includes it—the
complete request URL or query parameters. The app does not intentionally add
signing private keys, passwords, or SignPath tokens to diagnostics, but users
must still inspect and redact reports before sharing them.

应用会在本机保存设置、安装来源、更新状态、操作状态和诊断日志。用户复制的诊断报告可能
包含应用版本、操作系统与架构、更新源 host、安装状态、本地日志路径、错误文本；如果上游
工具把完整请求地址写入错误，还可能包含 URL query。应用不会主动把签名私钥、密码或
SignPath token 加进诊断，但用户分享前仍必须检查并删改。

Users should review and redact diagnostics before sharing them publicly.
Uninstalling Codex App Manager does not automatically delete data owned by the
separately installed Codex application; the uninstall UI explains its retention
scope.

用户公开分享诊断前应先检查并删改敏感内容。卸载 Codex App Manager 不会自动删除独立安装
的 Codex 应用所拥有的数据；卸载界面会说明实际保留范围。

## Contact and user choices · 联系方式与用户选择

Users can disable Codex startup/periodic checks, choose an update source and
proxy, and decide whether to install a Manager update. Report privacy or
security concerns through [GitHub Issues](https://github.com/Wangnov/Codex-App-Manager/issues).
Never post tokens, passwords, private keys, complete presigned URLs, or other
secrets publicly.

用户可以关闭 Codex 启动/定时检查、选择更新源与代理，并决定是否安装 Manager 更新。
隐私或安全问题请通过 [GitHub Issues](https://github.com/Wangnov/Codex-App-Manager/issues)
报告；请勿公开粘贴 token、密码、私钥、完整预签名 URL 或其他秘密。

See also the [code signing policy](./code-signing-policy.md).
