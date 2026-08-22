# Codex App Manager 安全审查报告

审查时间：2026-08-22（Asia/Shanghai）

审查对象：`main` / `fa871cfde3c35e45a08fa8dd32c84ef1be089678`

审查方式：GitHub Security/API 只读检查、依赖审计、Tauri/React/Cloudflare/Actions 关键路径静态审查、线上响应头抽查。审查阶段未执行利用，也未修改仓库或 GitHub 安全设置。

## 后续处置状态（2026-08-22）

- CAM-SEC-001：修复分支已移除全部审计 `continue-on-error`，固定 `cargo-audit`/`cargo-deny` 版本，并覆盖根工程、官网、Worker 与三个独立 engine lockfile；合并后将把 `Audit` 加入 main 必需检查。
- CAM-SEC-002：修复分支已对自定义 appcast 和其中每个 enclosure 做运行时 DNS 公网地址检查，使用 `curl --connect-to` 将每次实际连接钉到已检查 IP，并禁止自动重定向；TLS SNI 与证书主机名校验仍保持原域名。多地址域名会依次尝试全部已验证公网地址；HTTP/SOCKS 代理场景若本机无法解析，则经该代理访问固定的 Google Public DNS DoH JSON 端点，校验返回地址后再固定连接，既保留代理端 DNS 能力，也不把目标解析盲信给代理。
- CAM-SEC-003：已将 `brace-expansion`、`nanoid`、`event-listener` 更新到修复版本，Dependabot 覆盖扩展至所有 npm/Cargo 子工程。`glib` 经四个发布 target 反向依赖树复核均不可达后，已按 `not_used` 记录化关闭告警。
- CAM-SEC-004：已启用 GitHub Private vulnerability reporting、Dependabot security updates 与 CodeQL default setup（`extended`、远程威胁模型），并新增 `SECURITY.md`。首次三语言分析均成功；扩展本地威胁模型产生的 487 条告警中，485 条来自桌面/发布工具预期读取的 `temp_dir`、用户目录、命令行参数和 GitHub Actions 环境文件，切回远程边界后只保留 2 条。修复分支已消除这 2 条（Worker 前缀正则复杂度、仅用于 CSP 断言的未锚定测试正则），并额外移除发布资产摘要的动态对象属性写入。
- CAM-SEC-005：官网修复分支新增 Cloudflare Static Assets `_headers`，包含 CSP、`nosniff`、frame、Referrer 与 Permissions Policy，并在构建时校验所有 inline script hash。
- CAM-SEC-006：仓库已启用 Actions 完整 SHA 强制要求，并将可用 action 收紧为 GitHub-owned 加四个当前明确使用的第三方仓库。

## 执行摘要

未发现能够直接远程接管当前发布的 macOS/Windows 客户端的 Critical 或 High 级漏洞。当前最值得优先处理的是两项 Medium 问题：安全审计工作流不阻断合并或发布；macOS 自定义更新源仅检查初始 URL 的字面主机，后续 DNS 结果、重定向目的地以及 appcast 内的下载 URL 未做同等内网限制。

审查时 GitHub 有 1 条未关闭 Dependabot 告警（`glib 0.18.5`，GHSA-wrw7-89jp-8q8g）。它来自 Tauri 的 Linux GTK 条件依赖；对四个实际发布 target 运行反向依赖树均无输出，因此不进入本项目发布的 macOS/Windows 二进制。后续已按 `not_used` 附可达性证据关闭，避免告警长期失真。

## Medium

### CAM-SEC-001：安全审计不会阻断合并或发布

- Rule ID：REACT-SUPPLY-001 / CI-SEC-GATE-001
- Severity：Medium
- Location：`.github/workflows/ci.yml:79-102`
- Evidence：整个 `audit` job 设置 `continue-on-error: true`；`npm audit`、安全工具安装、`cargo audit`、`cargo deny` 每一步也都设置为可失败。main ruleset 只要求 `Frontend` 与两个 `Rust` job，不要求 `Audit (report)`。
- Impact：即使发现 High 级 npm 漏洞、Rust 内存安全告警或安全工具根本未成功安装，PR 与 main 仍可显示必需检查全绿并进入发版链路。本次本地 `npm audit` 已报告 2 个 High 级开发依赖告警，而该配置不会阻断。
- Fix：把工具安装固定到已审核版本并缓存；至少令已知可修复的 High/Critical 漏洞阻断。对仅 Linux 条件依赖、不可维护 GTK3 等预期告警，以带到期时间和理由的精确 ignore/target policy 管理，不要用整个 job 的 `continue-on-error` 吞掉。
- Mitigation：在完全转为阻断前，将审计结果生成 SARIF/Artifact，并设置单独的必需安全检查，只对有明确基线的新增告警失败。
- False positive notes：当前两个 npm High 告警均为 `dev: true`，`npm audit --omit=dev` 为 0；它们不是已发布客户端的运行时依赖，但仍属于构建/CI 供应链风险。

### CAM-SEC-002：自定义更新源可通过 DNS/HTTPS 重定向绕过内网目标限制

- Rule ID：SSRF-DEST-001
- Severity：Medium
- Location：`src-tauri/src/app/url_guard.rs:37-88`；`crates/codex-mac-engine/src/sys.rs:46-68`；`crates/codex-mac-engine/src/download.rs:127-169`；`src-tauri/src/app/mac_update.rs:161-176, 626-644`
- Evidence：`validate_custom_source` 只拒绝字面 IP 和若干域名后缀，不解析域名；`curl -L` 会自动跟随任意 HTTPS 重定向，且只限制协议为 HTTPS；appcast 中的 enclosure URL 在下载前未经过 `validate_custom_source` 或等效目的地检查。
- Impact：恶意或被接管的自定义 appcast 可以诱导客户端向本机/LAN/云元数据的 HTTPS 地址发起 GET，形成有限的 blind SSRF/跨网段请求。OpenAI Ed25519、codesign/Gatekeeper 闸会阻止伪造包安装，因此这里不是任意代码执行，但网络侧副作用仍存在。
- Fix：使用可检查每一跳的 HTTP 客户端；初始 URL、每次重定向和 appcast enclosure URL 都解析 DNS，并拒绝 loopback、link-local、private、ULA、CGNAT、documentation ranges 与重绑定后的受限地址；连接时绑定已验证地址并校验 TLS 主机名。更保守的方案是只允许官方/镜像 host allowlist，将任意自定义源置于明确的高级风险模式。
- Mitigation：暂时关闭自动跟随重定向，或仅允许同 host HTTPS 重定向；下载 URL 至少先复用一套严格的 scheme/host 校验。
- False positive notes：利用前提通常是用户主动选择恶意自定义源或该源被攻陷；默认 Mirror/Official/Auto 源不由普通远程输入控制。

## Low

### CAM-SEC-003：依赖告警与覆盖面不完整

- Rule ID：REACT-SUPPLY-001
- Severity：Low
- Location：`package-lock.json:1707-1715, 4535-4568, 4858-4862`；`.github/dependabot.yml:1-14`；`src-tauri/Cargo.lock`
- Evidence：根工程 `npm audit` 报告 `brace-expansion` 与 `nanoid` 两个 High advisory，均为开发依赖；生产依赖审计为 0。RustSec 另报告 `event-listener 5.4.1` 的 unsound 告警（修复版 `>=5.4.2`），与 `glib 0.18.5` 一样只出现在 Linux 条件依赖树。Dependabot 仅覆盖根 npm 与 `src-tauri` Cargo，未覆盖 `website`、Cloudflare Worker 与三个独立 engine lockfile。
- Impact：当前发布平台的直接风险较低，但构建工具可遭遇 DoS/异常行为；新增子工程依赖问题也可能不被自动发现。
- Fix：更新 lockfile 中可修复版本；扩展 Dependabot 到 `website/`、`cloudflare/manager-download-router/` 和三个 engine crate；CI 分别执行各 lockfile 的审计。
- Mitigation：保留 `npm ci` 与 SHA lock；针对 dev-only 告警记录可达性分析和修复期限。
- False positive notes：`glib`/`event-listener` 当前均不进入 macOS/Windows target tree；不要把 GitHub 的通用包严重级别直接等同于本项目发布风险。

### CAM-SEC-004：缺少协调披露入口与 CodeQL 分析

- Rule ID：SEC-GOV-001
- Severity：Low
- Location：仓库根目录无 `SECURITY.md`；GitHub API 返回 private vulnerability reporting disabled、code scanning `no analysis found`。
- Evidence：公开 Security 页面显示 “No security policy detected”；没有已发布 Security Advisories。Secret scanning 与 push protection 已开启，但 CodeQL 尚未产生分析结果。
- Impact：研究者没有明确的私密披露渠道；TypeScript、Rust 与 GitHub Actions 中可被静态分析发现的问题缺少持续检测。
- Fix：增加包含受支持版本、联系方式、响应时限的 `SECURITY.md`；启用 Private vulnerability reporting；为 JavaScript/TypeScript、Rust、GitHub Actions 启用 CodeQL default setup 或固定 SHA 的 advanced setup。
- Mitigation：至少在 README/Security 页面提供不公开泄露细节的联系方法。
- False positive notes：没有 published advisories 只表示没有公开发布，不等于没有漏洞。
- Remediation verification：CodeQL default setup 已对 JavaScript/TypeScript、Rust、GitHub Actions 完成成功分析；查询套件保持 `extended`。远程威胁模型复扫只留下 2 条可定位结果，均已在本修复分支修改并由对应测试覆盖。

### CAM-SEC-005：官网缺少常见浏览器安全响应头

- Rule ID：REACT-HEADERS-001 / JS-CSP-001
- Severity：Low
- Location：2026-08-22 实测 `https://codexapp.agentsmirror.com/` 响应头
- Evidence：首页 200 响应未见 `Content-Security-Policy`、`X-Content-Type-Options`、`frame-ancestors`/`X-Frame-Options`、`Referrer-Policy`、`Permissions-Policy`。
- Impact：静态官网当前未发现直接 XSS 数据流，因此即时风险低；一旦以后加入动态内容或第三方脚本，会缺少重要纵深防御，并可被跨站 frame 嵌入。
- Fix：在 Cloudflare/Sites 响应层统一设置 CSP、`nosniff`、`frame-ancestors 'none'`、合理的 Referrer/Permissions Policy；先以测试环境或 report-only 验证资源清单。
- Mitigation：继续自托管脚本和字体，避免无 SRI 的第三方脚本。
- False positive notes：这些头可能在其他路径单独设置；本结论仅针对实测首页响应。

### CAM-SEC-006：GitHub Actions 未在仓库设置层强制 SHA pin

- Rule ID：CI-SUPPLY-002
- Severity：Low
- Location：GitHub Actions repository permissions
- Evidence：`allowed_actions=all`、`sha_pinning_required=false`。当前仓库所有外部 action 均已手工固定完整 commit SHA，这是良好现状，但设置不阻止未来 PR 引入 tag 引用或新 action。
- Impact：未来一次工作流改动可能无意扩大第三方供应链风险，尤其发版 job 持有签名、对象存储和发布权限。
- Fix：启用 SHA pin requirement，或把 allowed actions 收紧到 GitHub-owned 与明确 allowlist；保留 Dependabot 对 action SHA 的更新。
- Mitigation：为 `.github/workflows/**` 增加 CODEOWNERS/强制审批，并让 CodeQL GitHub Actions 查询参与必需检查。
- False positive notes：这不是当前工作流已被攻陷；当前所有 `uses:` 已固定 SHA。

## 已确认的正向控制

- Tauri production CSP 的 `script-src` 仅允许 `'self'`，禁止对象、表单与 frame；顶层导航只允许精确 bundled origin。
- capability 清单较窄，未开放通用 shell；外部链接命令仅允许 HTTP(S)。
- 自更新使用内置 minisign 公钥；macOS 包使用 OpenAI Sparkle Ed25519、公证/Gatekeeper、Team ID 与 bundle ID 多重验证；Windows 使用 SHA-256、Authenticode 与 OpenAI/Microsoft Marketplace 身份钉扎。
- main ruleset 禁止删除和 non-fast-forward，并要求 Frontend、macOS Rust、Windows Rust 三项检查；release tag 另有创建授权与不可变规则。
- Secret scanning 与 push protection 已启用；仓库默认 `GITHUB_TOKEN` 权限为 read；当前第三方 actions 全部固定 SHA。
- 根生产 npm、官网 npm、Cloudflare Worker npm 以及三个独立 engine Rust lockfile 本次未发现已知运行时漏洞。

## 建议优先级

1. 先修 CAM-SEC-001，让可修复的新 High/Critical 安全告警真正阻断。
2. 再修 CAM-SEC-002，对自定义源每个 DNS/redirect/download 目的地复检。
3. 同批更新 npm dev 依赖与 `event-listener`，并补齐所有子工程扫描。
4. 补 `SECURITY.md`、Private vulnerability reporting、CodeQL 与官网响应头。
