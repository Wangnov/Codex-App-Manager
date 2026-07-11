# Windows signing and verification

Windows publisher trust, Tauri updater signatures, and Microsoft SmartScreen
reputation are independent mechanisms. A `.sig` file is not Authenticode, and a
valid Authenticode signature cannot guarantee immediate SmartScreen reputation.

Public policy: [code signing policy](./code-signing-policy.md) ·
[privacy policy](./privacy.md)

## 中文

### 当前状态（申请/迁移中）

- 项目正在申请 **SignPath Foundation**。截至 2026-07-11，申请尚未获批，证书尚未签发，
  GitHub trusted-build 签名集成尚未上线。
- 当前已发布的 Windows x64 / ARM64 安装器**没有 Authenticode 发行者签名**，首次运行
  可能出现 SmartScreen 提示。每个历史版本的实际状态以其 release note 为准。
- `.sig` / `latest.json` 中已有的 Tauri updater 签名只校验应用内更新下载到的字节，不会
  让 Windows 显示可信发行者。
- tag 工作流中的 Windows job 当前会在构建前执行
  `assert-signpath-foundation-ready.ps1` 并故意失败。因为发布 job 依赖所有 matrix build，
  所以不会发布缺少 Windows 或 unsigned Windows 的不完整 release。
- 仓库不再接受 `WINDOWS_CERTIFICATE`、PFX 密码或临时 Tauri thumbprint config。给
  environment 填入一个证书 secret 不能绕过阻断。

### 为什么没有直接接入 SignPath action

SignPath Foundation 的公开 GitHub 流程是：可信 GitHub Actions 产生工件 → 把工件提交给
SignPath → 对每次请求进行人工批准 → 下载签名后的工件。它不是本地 PFX，也不能直接当作
Tauri `signCommand` 使用。

当前 Tauri/NSIS 的 inside-out 签名路径会在打包期间处理主程序、卸载器、外层安装器，且
可能准备已签名的第三方 NSIS 插件副本；而 Foundation 证书只能用于本项目允许的开源
工件，不能把第三方插件冒充项目自有二进制直接签名。SignPath 对 PE 的工件模型也不能被
未经验证地当成会递归处理 NSIS 内嵌 payload。因此，本仓库不会先猜一个“两轮签名”顺序
并把它当作生产方案。

### 启用前必须关闭的阻塞项

一个独立 PR 必须提供真实的 SignPath 测试项目/证书证据，并完成：

1. **Trusted build**：签名请求绑定本公开仓库、受保护 tag、commit、workflow run 与准确
   版本；任何 PR 代码都拿不到签名凭据。
2. **人工批准**：每个 release 请求由明确的 SignPath approver 单独审核，不能自动批准。
3. **工件配置**：明确哪些文件属于本项目，排除第三方 NSIS/Tauri 插件直接签名；产品名
   与版本资源必须受 SignPath 规则约束。
4. **可复现试签**：对 x64 与 ARM64 进行试签，证明最终安装器、安装后的主程序和
   `uninstall.exe` 都得到预期的 SignPath Foundation 发行者身份及有效时间戳。
5. **NSIS 顺序**：用真实工件证明可行的打包/签名顺序；在证据出现前，不实现两轮签名
   流水线，也不声称 SignPath 会深入签署 NSIS payload。
6. **发布字节绑定**：在 Authenticode 完成后生成 Tauri updater `.sig`，再核对 GitHub
   Release、R2 与 IHEP 最终上传文件的 hash 与已验证工件一致。
7. **运行验收**：x64 完整执行 install → launch → upgrade → uninstall；ARM64 至少完成
   安装、签名、升级和卸载检查，并在 ARM64 设备或可信虚拟化上补主程序启动。

只有上述证据经过 review，才可以用已批准的 trusted-build action 替换 fail-closed 脚本。
仅删除阻断脚本仍不够：后续 `required` 验证会拒绝 `NotSigned`、错误发行者或缺少时间戳的
工件。

### 预留的验证工具

- [`scripts/assert-signpath-foundation-ready.ps1`](../scripts/assert-signpath-foundation-ready.ps1)
  — 当前正式发布硬阻断；故意没有成功路径或 secret 开关。
- [`scripts/verify-windows-authenticode.ps1`](../scripts/verify-windows-authenticode.ps1)
  — `required` 模式验证 Windows 报告的签名状态、预期 subject 与时间戳；`optional` 只供
  unsigned PR / 本地诊断。时间戳 countersigner 本身不足以证明所有 RFC3161 细节，试签还
  必须检查 SignPath policy 与 `signtool verify /pa /all /v` 输出。
- [`scripts/windows-packaged-smoke.ps1`](../scripts/windows-packaged-smoke.ps1)
  — 安装后检查主程序与卸载器，并覆盖升级/卸载路径。
- [`scripts/windows-pe-arch.ps1`](../scripts/windows-pe-arch.ps1)
  — 断言 x64 / ARM64 主程序的 PE machine type；cross-build 不等同于 ARM64 运行验证。

仓库中不存在生产可用的 SignPath token、organization/project slug、signing policy slug 或
artifact configuration slug。在申请获批前不要猜测这些值，也不要新增 PFX fallback。

### 用户如何核验现有版本

当前版本预期会显示 `NotSigned`；下面的命令用于核对实际文件，而不是把 updater `.sig`
误认成 Authenticode：

```powershell
Get-AuthenticodeSignature .\CodexAppManager_x64-setup.exe |
  Format-List Status,StatusMessage,SignerCertificate,TimeStamperCertificate
Get-FileHash .\CodexAppManager_x64-setup.exe -Algorithm SHA256
```

文件 hash 应与对应 GitHub Release 的 `SHA256SUMS` 比对。未来启用 SignPath 后，每个
release note 仍必须按真实验证结果说明该版的 Windows 签名状态。

## English

### Current state (application/migration pending)

- The project is applying to **SignPath Foundation**. As of 2026-07-11, the
  application has not been approved, no certificate has been issued, and no
  GitHub trusted-build signing integration is active.
- Published Windows x64/ARM64 installers are **not Authenticode-signed** today
  and may trigger SmartScreen. The release note for each historical version is
  the source of truth for that artifact.
- Existing Tauri updater signatures in `.sig` / `latest.json` authenticate the
  downloaded update bytes only; they do not establish a Windows publisher.
- Windows tag jobs intentionally fail before building by running
  `assert-signpath-foundation-ready.ps1`. The publish job requires the complete
  build matrix, so it cannot produce a partial or silently unsigned release.
- This repository no longer accepts `WINDOWS_CERTIFICATE`, PFX passwords, or a
  generated Tauri thumbprint config. Adding a certificate secret cannot bypass
  the blocker.

### Why there is no SignPath action yet

The public SignPath Foundation GitHub flow submits an artifact from a trusted
GitHub Actions build, requires manual approval for each request, and returns a
signed artifact. It is not a local PFX and cannot be used directly as Tauri's
`signCommand`.

Tauri/NSIS inside-out signing operates during packaging across the app,
uninstaller, outer installer, and potentially prepared copies of third-party
NSIS plugins. A Foundation certificate must not directly sign third-party
plugins as project-owned binaries. SignPath's PE artifact model must also not be
assumed to recursively sign an embedded NSIS payload without evidence. This
repository therefore does not implement or claim an unverified two-pass flow.

### Blockers that must be closed before activation

A separate PR must use a real SignPath test project/certificate to demonstrate:

1. trusted origin binding to this public repository, protected tag, commit,
   workflow run, and exact version, without exposing credentials to PR code;
2. manual approval of every release request by an explicit SignPath approver;
3. artifact rules that bind product/version metadata and exclude direct signing
   of third-party NSIS/Tauri plugins;
4. reproducible x64 and ARM64 pilot signing for the final setup executable,
   installed app, and `uninstall.exe`, with the expected SignPath Foundation
   publisher and valid timestamp;
5. a proven NSIS packaging/signing order rather than an assumed two-pass design;
6. a Tauri updater `.sig` created only after Authenticode, followed by hash
   equality across the verified artifact, GitHub Release, R2, and IHEP; and
7. x64 install/launch/upgrade/uninstall smoke plus native ARM64 launch evidence.

Only reviewed evidence can replace the fail-closed script with the approved
trusted-build action. Deleting the blocker alone is insufficient: downstream
required checks must still reject `NotSigned`, an unexpected publisher, or a
missing timestamp.

### Reserved verification tools

- `assert-signpath-foundation-ready.ps1` is the current hard blocker and has no
  success path or secret-controlled bypass.
- `verify-windows-authenticode.ps1` checks Windows signature status, expected
  subject, and timestamp in required mode; optional mode is for intentionally
  unsigned PR/local diagnostics. A timestamp countersigner alone does not prove
  every RFC3161 detail, so the pilot must also review SignPath policy and
  `signtool verify /pa /all /v` output.
- `windows-packaged-smoke.ps1` verifies installed PE files and exercises the
  package lifecycle.
- `windows-pe-arch.ps1` asserts the app PE architecture; cross-building ARM64
  does not replace a native ARM64 launch test.

There is no production SignPath token, organization/project slug, signing
policy slug, or artifact-configuration slug in this repository. Do not invent
those values or add a PFX fallback before approval.

### Verifying current releases

Current artifacts are expected to report `NotSigned`. Use
`Get-AuthenticodeSignature` to inspect the actual file and compare its SHA-256
hash with the matching release's `SHA256SUMS`; do not treat a Tauri updater
`.sig` as publisher identity. After SignPath activation, each release note must
still state that release's observed Windows signing status.
