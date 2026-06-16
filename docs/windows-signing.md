# Windows signing and verification

This project currently ships a Windows NSIS installer without Authenticode code
signing. The installer is still published through GitHub Releases and the
agentsmirror download mirror, and every release includes `SHA256SUMS` so users
can verify the bytes they downloaded.

## 中文

### 当前状态

- macOS 构建已经使用 Developer ID 签名并完成 Apple 公证。
- Windows 安装器 `CodexAppManager_x64-setup.exe` 当前没有 Authenticode 代码签名。
- Windows 应用内自更新包带有 Tauri updater 签名,用于校验下载字节没有被篡改。
- Windows 首次手动运行安装器时可能出现 SmartScreen 提示;这是预期风险,不是更新器签名失效。

### 三个概念

**Tauri updater 签名:** `latest.json` 里的 `signature` 对安装包字节签名。它保护应用内自更新下载,确保镜像或网络传输没有改包。它不参与 Windows 系统的发行者信任判断,也不能消除 SmartScreen 提示。

**Authenticode 代码签名:** Windows 对 PE 文件和安装器使用的代码签名体系。拥有 OV 或 EV 证书后,安装器可以显示发行者身份,并更容易建立系统信任。本项目当前还没有给 Windows 安装器做 Authenticode 签名。

**SmartScreen 信誉:** Microsoft Defender SmartScreen 会结合签名身份、下载量、历史信誉和风险信号做拦截判断。EV 证书通常能更快建立信誉;OV 证书和未签名分发都需要累积信誉,未签名安装器首次运行更容易被提示。

### 如何核验下载

1. 从 [GitHub Releases](https://github.com/Wangnov/Codex-App-Manager/releases/latest) 或 agentsmirror 镜像下载对应安装包。
2. 从同一个 release 的 Assets 下载 `SHA256SUMS`。
3. 在本机计算哈希并与 `SHA256SUMS` 中的同名文件比对。

Windows PowerShell:

```powershell
Get-FileHash .\CodexAppManager_x64-setup.exe -Algorithm SHA256
```

macOS:

```bash
shasum -a 256 CodexAppManager_aarch64.dmg
shasum -a 256 CodexAppManager_x86_64.dmg
```

如果哈希不一致,不要运行该文件,请重新下载或在 issue 中反馈下载来源和文件名。

### 分发渠道与成本评估

**GitHub Releases + agentsmirror + SHA256SUMS:** 这是当前主渠道。优点是透明、可回溯、可独立核验;缺点是 Windows 无 Authenticode 时仍可能触发 SmartScreen。

**winget:** 仓库已有自动提交流程,但 `Wangnov.CodexAppManager` 只有在 microsoft/winget-pkgs 的首包 PR 合并后才真正可用。在那之前,文档不应提供 winget 安装命令。winget 可接受未签名 NSIS 安装器,但新发布者首包通常会有额外人工审查。

**Microsoft Store / Partner Center:** 这是中期可选路径,需要开发者账号、MSIX 打包和商店审核。优点是用户信任更强,缺点是流程和维护成本更高。

**OV / EV 代码签名证书:** 证书不是当前发布阻塞项。后续可在 Windows 下载量、企业用户需求或支持成本达到明确阈值后重新评估。EV 成本更高但信誉建立更快;OV 成本较低但仍需累积 SmartScreen 信誉。

**不推荐的降本方式:** 不把私钥托管给不可信第三方,不合租硬件令牌,不把代码签名能力转交给无法审计的渠道。

### 风险披露

未签名 Windows 安装器意味着用户首次运行可能需要在 SmartScreen 中选择更多信息后继续。项目通过公开 release、镜像直链、`SHA256SUMS`、Tauri updater 签名和透明文档降低篡改与误解风险;这不能替代 Authenticode,但可以让用户在证书预算到位前做独立核验。

## English

### Current status

- macOS builds are Developer ID signed and Apple notarized.
- The Windows installer `CodexAppManager_x64-setup.exe` is not Authenticode-signed yet.
- Windows in-app update artifacts carry the Tauri updater signature, which verifies the downloaded bytes.
- SmartScreen may warn when users manually run the Windows installer for the first time; that is the known distribution risk, not an updater-signature failure.

### Three separate concepts

**Tauri updater signature:** The `signature` field in `latest.json` signs the installer bytes. It protects in-app self-update downloads from tampering across mirrors and network hops. It is not Windows publisher trust and does not remove SmartScreen warnings.

**Authenticode code signing:** This is the Windows code-signing system for PE files and installers. With an OV or EV certificate, the installer can show a publisher identity and build operating-system trust. This project does not currently Authenticode-sign the Windows installer.

**SmartScreen reputation:** Microsoft Defender SmartScreen combines signing identity, download volume, historical reputation, and risk signals. EV certificates usually establish reputation faster; OV certificates and unsigned distribution still need reputation to build, and unsigned installers are more likely to warn on first run.

### How to verify downloads

1. Download the installer from [GitHub Releases](https://github.com/Wangnov/Codex-App-Manager/releases/latest) or the agentsmirror mirror.
2. Download `SHA256SUMS` from Assets on the same release.
3. Compute the local hash and compare it with the matching filename in `SHA256SUMS`.

Windows PowerShell:

```powershell
Get-FileHash .\CodexAppManager_x64-setup.exe -Algorithm SHA256
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

**winget:** The repository has an automated submission workflow, but `Wangnov.CodexAppManager` only exists after the first package PR is merged in microsoft/winget-pkgs. Until then, the docs should not publish a Windows Package Manager install command. winget accepts unsigned NSIS installers, but a new publisher's first package usually receives extra manual review.

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
