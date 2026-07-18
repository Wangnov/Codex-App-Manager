# Codex App Manager — 产品设计（v1）

> 本文是 `grill-me` 设计拷问后的总纲，沉淀全部已拍板决策、服务端契约演进、平台机制、信任模型、分发与实施计划。
> 它扩展并统领骨架里的 [`architecture.md`](./architecture.md) 与 [`manifest-contract.md`](./manifest-contract.md)。

## 1. 定位与边界

- **是什么**：一个独立的 Tauri 桌面客户端（Win + macOS），负责 Codex 桌面应用的**安装、更新、卸载**生命周期，把 [`codex-app-mirror`](https://github.com/Wangnov/codex-app-mirror) 作为**权威服务端源**。
- **解决什么**：受限地区 / 受限 Windows 机器的用户，已经从 mirror 拿到离线包，现在希望**在本地直接更新**，而不是每次重新拉离线镜像；并尽量贴近官方"应用内更新"的顺滑体验。
- **v1 职责边界**：**纯生命周期**（装 / 更 / 卸）。`~/.codex` 家目录**只在卸载时**作为"默认保留 / 可选清除"的对象，v1 不做配置 / auth / MCP / 历史管理。
- **北极星（非 v1）**：架构上预留扩展位，未来可能演进为更完整的 Codex 管理中枢，但**不以此为 v1 目标**。
- **明确不做**：不修改 Codex 安装包、不破解商店 / OpenAI 授权、不绕过本机 AppX/MSIX 策略、不替代官方分发渠道。

## 2. 决策清单（本次拷问结论）

| # | 分支 | 决策 | 关键理由 |
|---|------|------|---------|
| A | 纳管边界 | **混合纳管**：发现外部安装（商店 / 官方 DMG / 手动离线包）即按"技术正确方式"接管；首次成功操作后写 provenance，转为自管 | 兼顾真实场景（用户多半已有 Codex）与技术约束（商店 MSIX 无法纯旁路更新） |
| A | 接管触发 | **显式同意**：主动检测 + UI 告知来源与将发生的动作，用户确认后才动 | 修改用户已有的官方安装，对安全敏感人群是信任红线 |
| C | 更新策略 | **mac = 复用 OpenAI Sparkle delta**（一版落后 18MB/全量 406MB）；**Win = 自研增量**（α 全量起步 + 预埋逐文件哈希）；统一 staging→校验→原子替换→重启 | mac 官方有 delta、Win 无（见 §3/§6/§9）|
| D | Win 安装模型 | **自动择优 + 运行时失败回退 + 透明**：优先 MSIX 侧载，失败自动回退便携，清楚告知损失；高级用户可强制 | 两类人群（个人机 / 锁定机）都最优 |
| D | Win 信任姿态 | **不提权、不改系统策略**：MSIX 侧载只在"开箱可侧载"时进行，否则判定不可用 → 回退便携 | 不弹 UAC、不动系统信任；锁定机也不徒劳 |
| D | 便携补偿 | **非侵入**：开始菜单快捷方式 + Apps&Features 卸载项 + provenance；**跳过文件关联**；`codex://` 协议交给 app 自注册 | 便携核心（登录 + 启动）已验证无障碍（见 §5.3） |
| E | macOS 引擎 | **自建 Sparkle-style delta 引擎**：读镜像 appcast→EdDSA 验→应用 OpenAI delta/全量→同卷原子替换→重启；**跟随现有位置 + 免授权**；运行中=优雅退出后 swap | 复用 OpenAI deltas、manager 掌控；详见 §6 |
| B | 通道 | **仅 stable**；manifest **预留 `channel` 维度** | 聚焦；beta 基础设施已存在（store `9N8CJ4W95TBZ`），以后加探测即可 |
| F | 分发 / 自更新 | 走 **mirror R2/S3 同一轨道**（CN 可达）+ GitHub 全球源 + 落地页；**Tauri updater**（minisign，"提示后更新"） | 复用 mirror 已解决的 CN 分发；manager 自身只有几 MB |
| F | manager 签名 | **Mac 公证**（已有开发者证书）/ **Win 暂不签**（SmartScreen 缓解，后续补证书） | 成本约束；Mac 必需，Win 早期可忍 |
| G | 信任锚 | **原生 OpenAI 签名为主锚** + sha256 完整性 + HTTPS manifest；签名 manifest 留作加固 | 即便 mirror/CDN 被攻陷，攻击者也伪造不了 OpenAI 代码签名 |
| H | 范围 | v1 纯生命周期，`~/.codex` 仅卸载时保留/清除 | 见 §1 |
| J | 更新姿态 | **静默后台下载 + 就绪提示**（浏览器式），但**配护栏**：默认仅非计量网络自动下载、断点续传、有总开关 | 用户选丝滑优先；护栏保护受限带宽人群 → **β 增量优先级上调** |
| J | 形态 | **窗口应用**（非纯托盘）；开机自启 / 后台检查**默认关、可选开** | 生命周期操作需清晰操作面 + 体检报告 + 进度 |
| — | 发布次序 | **两端一起跑通**（v1 双平台全生命周期）；**Win MSIX 侧载为唯一长杆**，便携路径必达、MSIX 必要时小幅 fast-follow | 用户要平台齐发；以便携保底避免被 MSIX 拖死 |
| I | 技术底座 | **Tauri v2 + 保留 hexagonal 后端骨架**（弃前端），补 updater 插件与真实 adapters | 骨架的 managed/external + 策略枚举已与决策吻合 |

## 3. 关键事实：Codex 官方更新机制（实测自 asar）

> 决定了"增量"到底是什么。证据来自解包的 `resources/app.asar`（v42.1.0 壳，Codex 26.519.x）。

- **技术栈**：Electron + **Electron Forge**（`maker-squirrel` / `maker-zip` / `maker-msix`）。**不是** electron-builder/electron-updater —— `electron-updater`/`MacUpdater`/`differentialDownload`/`latest-mac.yml`/`quitAndInstall` 全为 0。
- **Windows = 跳商店**：asar 内写死
  ```js
  Prod: { kind:'store', storeProductId:'9PLM9XGG6VKS',
          storeUpdateManifestUrl:'https://persistent.oaistatic.com/codex-app-prod/windows-store-update.json' }
  ```
  app 读该 JSON（仅 `buildVersion`/`storeProductId`/`packageIdentity`）发现新版 → **引导到微软商店**。块级增量是**商店基础设施**给的，不是 app 自下。对被墙用户此路本断 —— **我们是替代它**。
- **macOS = Sparkle 2.9.1**（实测自 `.app` bundle + appcast，**非** Squirrel.Mac；asar 内 `codexSparkleFeedUrl` / `sparkleManager` / 原生 `sparkle.node`）：读 `appcast.xml` → EdDSA 验签 → 下载 **delta 或全量 zip** → `Installer.xpc` 安装 → 重启。**OpenAI 已发布二进制 delta**（见 §6）。
- **结论**：mac「跟官方一致」**本就是 delta**（一版落后只下 **18MB** / 全量 **406MB**）；只有 Windows 才是“无 app 自更新、跳商店”。故 mac **复用 OpenAI 的 Sparkle delta**，Windows **自研增量**（见 §9）。
- **登录**：主登录走 **loopback**（`redirect_uri` + `127.0.0.1` + PKCE/S256），**不依赖 `codex://`**；`codex://` 仅用于 connector OAuth 回调 / deeplink，且 win32 运行时由 `setAsDefaultProtocolClient('codex')` **自注册到 HKCU**。→ 便携版登录与协议均无障碍。
- **负载体量**（解包后）：`codex(.exe)≈220–240M`、`app.asar≈159M`、`node.exe≈91M` + Chromium 框架。**稳定**（跨 patch 少变）：Chromium 框架、`node.exe`；**易变**：`app.asar`、`codex` 二进制。

## 4. 服务端契约演进（mirror 要新增什么）

mirror 维持「GitHub Release 存全量历史 + R2/S3 短链只存 latest + download-router 按国家分流」不变。**新增**：

### 4.1 manifest v3（在现有 v2 基础上扩展）

```jsonc
{
  "schemaVersion": 3,
  "channel": "stable",                 // 预留维度（v1 恒为 stable）
  "sources": { /* 保留 v2: windows / macos.arm64 / macos.x64 */ },
  "derived": { "windowsVersion": "26.602.3474.0" },

  "manager": {
    "minManagerVersion": "1.0.0",      // 服务端可声明最低兼容客户端
    "payloads": {
      "windows": {
        "url": "https://codexapp.agentsmirror.com/latest/win",
        "format": "msix",
        "sha256": "…", "size": 0,
        "moniker": "OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0",
        "signature": { "kind": "authenticode", "subject": "CN=OpenAI, …" }
      },
      "macos": {
        "arm64": { "url": ".../latest/mac-arm64", "format": "dmg", "sha256": "…", "size": 0,
                   "bundleId": "com.openai.codex", "teamId": "…",
                   "shortVersion": "26.602.30954", "bundleVersion": "3575" },
        "x64":   { "url": ".../latest/mac-intel",  "format": "dmg", "sha256": "…", "size": 0, "…": "…" }
      }
    },
    "fileManifests": {                  // β 增量预埋（v1 可先只产出、不消费）
      "windows":    ".../latest/win.files.json",
      "macosArm64": ".../latest/mac-arm64.files.json",
      "macosX64":   ".../latest/mac-intel.files.json"
    }
  }
}
```

### 4.2 逐文件清单 `*.files.json`（β 增量基石）

```jsonc
{
  "schemaVersion": 1,
  "payloadVersion": "26.602.3474.0",
  "root": "app",                       // 解包后相对根
  "files": [
    { "path": "resources/app.asar", "sha256": "…", "size": 166723584 },
    { "path": "Codex.exe",          "sha256": "…", "size": 1234567 }
    // … 每个文件一条
  ]
}
```
- CI 在打包时顺手产出（MSIX 本是 zip、可枚举条目；mac 端从挂载的 `.app` 树枚举）。
- **β 增量的 mac 取数不对称**：DMG 不易 Range 抽取内部文件 → β 阶段 mirror 需为 mac **额外发一份可寻址负载**（zip / 平铺树）。v1 先不发，留到 β。

### 4.3 manager 自更新 feed（Tauri updater 格式）

托管在 `…/manager/latest.json`（R2/S3 + GitHub）：
```jsonc
{
  "version": "1.0.0",
  "pub_date": "2026-06-05T00:00:00Z",
  "notes": "…",
  "platforms": {
    "darwin-aarch64": { "url": ".../manager/CodexAppManager_1.0.0_aarch64.app.tar.gz", "signature": "<minisign>" },
    "darwin-x86_64":  { "url": ".../manager/CodexAppManager_1.0.0_x64.app.tar.gz",    "signature": "<minisign>" },
    "windows-x86_64": { "url": ".../manager/CodexAppManager_1.0.0_x64-setup.nsis.zip","signature": "<minisign>" }
  }
}
```

### 4.4 macOS Sparkle 镜像（mac 增量基石，新增）

mirror 现仅镜像 `Codex.dmg`（够全新安装）。mac 增量需**额外镜像 Sparkle 更新源**：
- `appcast.xml`（**重写 enclosure URL 指向 mirror/R2-S3**，保留 EdDSA 签名原样）。
- 全量更新 `Codex-darwin-arm64-<ver>.zip`（latest）。
- **delta 文件** `Codex<新build>-<旧build>-arm64.delta`（保留最近 ~5 版窗口，体积小）。
- 同理可扩展 x64 与（未来）beta `codex-app-beta` 通道。

> 收益：mac 增量**复用 OpenAI 已算好的 delta**（一版落后 18MB），无需自研文件级 diff。Windows 无此等价物，仍需自研（§9）。

## 5. Windows 安装 / 更新机制

### 5.1 能力检测 → 择优（核心绕开"商店能不能下"——包永远从 mirror 拉）

探测信号（只读，不改系统）：
1. App Installer / AppX 部署可用性（`Add-AppxPackage` 能力、AppXSvc）。
2. 侧载策略：`HKLM\…\Appx\AllowAllTrustedApps`（被组织关掉 = 侧载大概率失败）。
3. 现有 Codex 安装与来源（商店托管 / 便携 / 无）→ provenance。
4. 体检报告把以上结论透明呈现给用户。

> 现代 Win10(1709+)/Win11 上签名良好的 MSIX **默认开箱可侧载**；"已被管理员阻止" = 组织关了策略或证书不受信。检测无法 100% 预判 → **采用"尝试→失败优雅回退"**而非"完美预测"。

### 5.2 MSIX 侧载路径（首选，保真度最高）
- 拿到真 package identity、`codex://`、文件关联、Apps&Features、**App Installer 原生差量更新**。
- **正常路径不提权、不装证书、不改策略**：AppX 事件日志探测仅用于识别冲突，探测超时或日志损坏时仍直接尝试已验签的本地包；仅在 Windows Update 正占用同一 Codex package moniker、导致本地包无法进入 AppX 队列时，请求一次 UAC，精准暂停该事务并立刻安装镜像 MSIX，安装完成即恢复相关服务。精确冲突下拒绝 UAC 会保留 staging 包并返回可重试的权限错误，不改走便携版，因为这不是 MSIX 能力缺失。
- 更新：同发布者、同 identity 的**新版签名 MSIX** 侧载即就地升级（天然覆盖商店版）。

### 5.3 便携解包路径（保底，普适、免管理员）
- 解包 MSIX（本是 zip）到 `%LOCALAPPDATA%\Programs\Codex`，跑 `Codex.exe`。零策略依赖。
- **已验证**：主登录走 loopback、协议 app 自注册 → **核心功能无障碍**。
- **补偿（纯 HKCU）**：开始菜单快捷方式、Apps&Features 卸载项、provenance；**默认不抢**电子表格文件关联；桌面快捷方式可选。
- **运行中更新**：Win 不能覆盖运行中文件 → 关闭 Codex → 替换 → 重启。下载到 staging、校验后再原子替换；保留旧版用于回滚。

## 6. macOS 安装 / 更新机制（Sparkle delta 引擎，实测）

> 实测自真实 `Codex.dmg`（arm64，26.602.30954/3575，406MB）+ appcast.xml。**自建 delta 引擎**：复用 OpenAI 的 Sparkle 产物，manager 全程掌控。

**实测关键事实**
- 签名：`Developer ID Application: OpenAI OpCo, LLC (2DC432GLL2)` → Apple Root；`com.openai.codex`；`codesign --verify --deep --strict` 通过、`spctl` = Notarized Developer ID accepted。
- DMG 由 curl 下载**无 `com.apple.quarantine`**（仅 benign `com.apple.provenance`）、挂载无 SLA 闸门。
- 更新源（asar 内运行时设定）：`appcast` = `https://persistent.oaistatic.com/codex-app-prod/appcast.xml`；`SUPublicEDKey`/`codexSparklePublicKey` = `mNfr1v9t63BfgDtlw4C8lRvSY6uMggIXABDOCi3tS6k=`；Sparkle 2.9.1 全套（`Autoupdate`/`Updater.app`/`Downloader.xpc`/`Installer.xpc`）。
- appcast 实有 **delta**：每版带前 ~5 版 delta，`Codex<新build>-<旧build>-arm64.delta`；**3511→3575 仅 18MB / 全量 406MB**。更新用 **`.zip`(+delta)** 而非 DMG（DMG 仅全新安装）。

**全新安装**：下载 DMG（或全量 zip）→ 校验 sha256 + codesign TeamID `2DC432GLL2` + Gatekeeper → 挂载 → `ditto` 拷出 `.app`（保签名/xattr）→ 放置 → 启动。位置：**跟随现有安装**；全新默认 `/Applications`（管理员通常免认证可写），不可写退 `~/Applications`；**绝不强制弹授权**。

**增量更新（自建引擎）**
1. 读**镜像 appcast** → 据已装 build 找 delta（窗口内）否则全量 zip。
2. 下载 delta/zip → **EdDSA 验签**（钥匙 `mNfr1v9t…`，**下载即验、应用前**）。
3. 以**已装 `.app` 为基准**用 Sparkle 开源 **`BinaryDelta apply`** 生成新 `.app`（落在同卷 staging）。
4. **应用后再验**新 `.app` 的 codesign + TeamID + 公证（最终闸门）。
5. **运行中替换**：检测到 Codex 运行 → 提示 → **优雅退出**（保护可能在跑的 agent，绝不强杀）→ 同卷 `rename()` 原子替换（旧 `.app` 留作回滚）→ 重启 → 启动健康检查通过后丢弃回滚。
6. **窗口外/应用失败**：超出 delta 窗口或 `BinaryDelta`/验签失败 → 回退全量 zip。

**约束**：**绝不修改已装 bundle 内部**（保持 pristine，否则 delta 应用校验必失败）；provenance 存在 bundle 之外（manager app-support 目录）。

**BinaryDelta 工具**：manager 需 **vendor Sparkle 开源 `BinaryDelta`**（MIT，版本对齐 OpenAI 的 2.9.1；实测 delta 格式 = *Patch v4.2 / LZMA*）。Codex 自带框架只有 `Autoupdate`、无独立 CLI，故 manager 自带一份。

**✅ 已端到端实测闭环（真机真数据）**：真实 3511 全量（404,477,775 B）+ 真实 18MB `Codex3575-3511-arm64.delta` → `BinaryDelta apply` → 还原出的 `.app`：`CFBundleVersion=3575`、`codesign --verify --deep --strict` 通过、`TeamIdentifier=2DC432GLL2`、`spctl` Notarized Developer ID 接受。codesign `--deep --strict` 通过即**字节级精确重建**。至此 mac delta 链路全验，剩余仅“优雅退出 + 同卷原子替换 + 回滚”纯工程。

**Codex 自带 Sparkle 共存**：其 feed 指向 oaistatic，对被墙用户哑。有 `CODEX_SPARKLE_ENABLED` env 可在 manager 启动 Codex 时关闭其自更新以避免“双更新源”（非被墙用户场景）；默认策略待定（§13）。

## 7. 信任与校验
- **主锚 = 包自身的 OpenAI 原生签名**：Win Authenticode 发布者 = OpenAI；mac codesign **TeamID `2DC432GLL2`** + 公证。用 OS API 验，攻击者伪造不了。
- **mac 更新额外锚 = Sparkle EdDSA 公钥** `mNfr1v9t63BfgDtlw4C8lRvSY6uMggIXABDOCi3tS6k=`：验 appcast 的 delta/zip 制品（下载即验）；应用 delta 后再用 codesign 复验结果 `.app`。
- **完整性**：sha256 比对 manifest（防损坏 / 传输错误）。
- **manifest 真实性**：v1 走 HTTPS；**加固项（后续）**：mirror 私钥签 manifest、manager 内嵌公钥校验，防篡改 manifest 做降级 / 元数据攻击。
- **失败处理**：任何校验失败 → 不触碰安装根 → 保留 staging 供重试 → 明确报错。
- **原子性 / 回滚**：下载 staging → 校验 → 原子替换 → 启动健康检查 → 通过后才丢弃旧版回滚材料。

## 8. manager 自身分发与自更新
- **首下（bootstrap）**：托管于 R2/S3（CN 可达）+ GitHub Release（全球）+ `agentsmirror` 落地页。
- **自更新**：Tauri updater 读 `…/manager/latest.json`，全量小包、minisign 校验，**"检查到→提示→更新"**（manager 自身非大流量，无需静默）。
- **签名**：Mac 公证（已具备）；**Win 暂不签** → 落地页给清晰指引（"更多信息→仍要运行"）+ sha256/minisign 自证 + 随下载量积累 SmartScreen 信誉；有预算再补 Win 证书。
- **版本流独立**：manager 与 Codex 负载各自独立版本；manager 读 `manager.minManagerVersion` 自检兼容。

## 9. 增量更新路线（mac 与 Win 走不同路）

**macOS —— 复用 OpenAI Sparkle delta（v1 即可达官方增量）**
- mirror 镜像 appcast + zip + delta（§4.4），manager 自建引擎应用 delta（§6）。
- 一版落后 **18MB / 全量 406MB**；超窗口回退全量。**无需自研 diff**。

**Windows —— 自研增量（无官方等价物）**
- **α（v1）**：全量下载 + 关-换-重启（= 商店替代）。manifest **预埋逐文件哈希**。
- **β（紧随，因“静默自动下载”上调）**：**文件级增量** —— 比对“已装文件树哈希 vs 新版 `*.files.json`”，只下哈希变化文件；省掉稳定的 Chromium 框架 / `node.exe`。MSIX 本是 zip → 按中央目录 Range 抽取条目。
- **γ（v2 可选）**：对 `app.asar` / `codex` 等易变大文件再做**块级 / zsync** 复用。需 mirror 发 zsync 控制数据 + 可 Range 托管。

**净结果**：mac 增量**起点就到位**（白嫖 OpenAI delta）；Windows 分阶段自研（α→β→γ）。

## 10. UX / 更新姿态
- **形态**：窗口应用。主面 = 能力体检报告 + 当前安装状态(managed/external/none) + 装/更/卸操作 + 进度。
- **更新姿态**：**静默后台下载 + 就绪提示**，护栏：
  - 默认**仅非计量网络**自动下载（Win 可测 metered；mac 较难 → 给开关）；
  - **断点续传 / 可暂停**；不打扰的进度；**一个总开关**关掉自动下载。
- 开机自启 / 后台版本检查：**默认关、可选开**。

## 11. 与骨架的映射（保留后端 / 弃前端）
- **保留**：`domain/`（`InstallationStatus = managed/external/…`、`OperationStrategy = windows-msix-preferred / windows-fixed-path-unpacked / macos-dmg-replace / managed-uninstall`）、`ports/`、`adapters/`、`app/planner` 的"出计划"分层。
- **扩展 domain**：`ManagedInstallation` 增 `source: store|portable|mirror-msix|official-dmg|unknown`（驱动纳管 / 接管逻辑）。
- **新增 ports**：`CapabilityProbe`(Win 检测)、`PayloadDownloader`(断点续传/Range)、`Verifier`(sha256 + 原生签名)、`Extractor`(MSIX 解包 / DMG 挂载)、`ProvenanceStore`(持久化纳管状态)、`SelfUpdater`(Tauri)。
- **从"出计划"到"执行"**：planner 产出 plan/steps，新增各平台 executor adapter 真正执行。
- **弃用**：现有前端整体重写。

## 12. 分阶段实施计划（两端齐发）
1. **共享核心**：manifest v3 客户端、版本对账（读 mirror manifest 当预言机）、provenance store、下载（断点续传/Range）、校验（sha256 + 原生签名）、staging/原子替换/回滚框架。
2. **macOS 全链路**（最快端到端验证核心 + 自更新 + 分发）。
3. **Windows 便携路径**（普适、必达）：检测、解包、HKCU 补偿、关-换-重启。
4. **Windows MSIX 侧载路径**（长杆）：开箱检测、侧载、就地升级；**必要时小幅 fast-follow**。
5. **manager 自更新 + 分发**：Tauri updater、R2/S3 托管、落地页、Mac 公证。
6. **β 文件级增量**：mirror 产出/托管可寻址负载 + 客户端文件级 diff。

## 13. 开放风险与待办
- **Win 未签名 → SmartScreen**：落地页指引 + 自证 + 信誉积累；预算到位补证书。
- **静默自动下载 vs 受限带宽**：靠 metered 检测 + 总开关 + 尽快上 β 缓解；mac metered 检测受限需替代方案。
- **MSIX 侧载证书信任**：部分机器签名不受信 → 归入"侧载不可用 → 回退便携"，**不**主动装证书。
- **β mac 取数不对称**：需 mirror 额外发 mac 可寻址负载，增量 CI 复杂度。
- **接管商店版后双更新源**：被墙环境商店哑、无冲突；非被墙环境需每次重读版本对账。
- **待确认**：§11 技术底座取舍（保留后端骨架）是否认可；落地页域名与 manager 安装包托管路径命名。
