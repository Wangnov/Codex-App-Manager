# Issue #343：新版 Windows 便携启动调查

调查日期：2026-09-18。Issue：https://github.com/Wangnov/Codex-App-Manager/issues/343

## 结论

对已实测的 Codex `26.915.31029` / MSIX `26.915.3509.0`，首选方案是：
**原生便携启动器为子进程设置指向随包 `resources/codex.exe` 的 `CODEX_CLI_PATH`，走上游已有的非应用包核心启动路径。**

这保留了真正的免注册启动与可移动程序目录，无需修改官方 EXE、ASAR、包清单或安装证书。
它解决了本次启动故障；尚不能据此宣称登录后所有功能、全部 Windows 版本和未来上游版本都兼容。

## 已确认的触发链

官方 ASAR 的 `package.json` 包含 `codexWindowsAppContainedCore: "1"`。
启动函数 `LA()` 的条件包含 Windows、`app.isPackaged`、存在 resourcesPath、没有非空
`CODEX_CLI_PATH`，以及该 metadata 标志。此处 `app.isPackaged` 不等于 Windows 进程具有包身份。

该分支在主程序加载之前调用 `windows-updater.node` 的 `getCurrentPackageFamily()`。
外层捕获异常并显示 `${app.getName()} failed to start.`。便携解包没有提供 Windows 包身份。

实测默认启动记录 `Desktop bootstrap failed to start the main app phase=bootstrap-import-main`，
且没有主页面 CDP target。通过 Win32 `GetPackageFullName` 查询默认启动与修正启动的两个
主进程，均返回 `15700`（`APPMODEL_ERROR_NO_PACKAGE`）。因此修正路径成功并不来自继承了
本机已安装 Codex 的包身份。

显式设置 CLI 后，启动条件中的应用包核心分支关闭，后台使用随包 CLI，通过 stdio 建立连接。
这是本版本上游已存在的代码路径，不是向 Windows 伪造官方包身份；未发现公开的稳定兼容性承诺。

## 对照实验

使用本机已安装的相同版本官方 payload 的独立副本。各组使用独立的 Codex home 和界面 profile，
没有复制用户登录凭据。调试端点使用动态端口并限制到 loopback。

| 实验 | 操作 | 结果 |
| --- | --- | --- |
| 默认便携启动 | 不设置 CLI override | bootstrap 失败；主页面 target 数量 0；进程仍存活 |
| 指定随包 CLI | 子进程环境设置 `CODEX_CLI_PATH=<副本>/resources/codex.exe` | 登录页 DOM ready=complete；app-server connected |
| 原生启动器 | 从自身目录定位 GUI 和 CLI，原样转发参数 | 登录页及 app-server 正常 |
| 移动程序目录 | 改到包含中文、空格、单引号和 `&` 的目录后启动原型 | 成功；日志中 CLI 路径跟随新目录 |

成功组确认：

- 页面文本为“登录 ChatGPT / 继续登录 / 使用其他方式登录 / 注册”。
- app-server 报告 `0.155.0-alpha.9`，初始化后进入 `connected`。
- `config/read`、`account/read`、`model/list`、`mcpServerStatus/list` 等请求成功响应。
- `computer-use native pipe startup ready`；随包 node、node-repl、Swift helper 路径正常解析。
- 未修改官方 `ChatGPT.exe`、`resources/app.asar`、`resources/codex.exe`；复制件与来源做 SHA256 对照。

原型已收敛为 `crates/codex-win-engine/src/portable_launcher.rs`，由该 crate 的
`build.rs` 按目标架构编译并嵌入。正式实现覆盖安装暂存、更新回滚、Manager 启动和开始菜单快捷方式；
旧安装从 Manager 启动时会生成/刷新启动器。共享 `portable_command.rs` 为 GUI 子进程配置随包 CLI。

新版健康检查除了最短存活时间，还要求出现主窗口；无主窗口的 bootstrap 错误会在有界期限内失败。
可读取到的原生启动错误弹窗会提前失败；实测默认失败路径被“30 秒没有主窗口”拒绝。
诊断构建示例（配置文件由 Manager 写入，不能只复制单个 EXE）：

```powershell
rustc --edition=2021 -O crates/codex-win-engine/src/portable_launcher.rs -o '<portable-root>/LaunchCodex.exe'
```

本机实验日志、截图及启动脚本保存在 `%TEMP%/codex-issue-343-lab/`。
测试结束关闭实验进程，清除由实验实例写入的 Chrome native-host discovery 条目，并将共享
native-host manifest 中的实验路径恢复到已有正式实例的有效路径。

## 方案比较

| 方案 | 身份/目录行为 | 判断 |
| --- | --- | --- |
| 随包 CLI + 原生启动器 | 无需注册；目录可移动；官方文件不变 | 本版本便携模式首选，已做上述实测 |
| 普通官方 MSIX | Windows 管理官方身份、服务和包扩展 | 完整系统集成的首选安装方式 |
| 原始解包目录注册 | 开发部署路径；需要正确完整布局和注册条件；受现有同包身份影响 | 不适合作为面向普通用户的透明便携修复，未对当前正式实例做替换实验 |
| 稀疏身份包 / external location | 支持外部目录，但要注册、处理签名及 EXE 身份关联 | 可用于自有程序；不是给任意官方 EXE 添一个 XML 就能完成 |
| 修改 ASAR / 修改 EXE manifest | 修改第三方内容，增加升级维护和签名问题 | 当前已有不修改内容的可行路径，不选 |
| 设置用户/系统全局 CLI 环境变量 | 固定路径会影响其他版本和安装方式 | 不选，仅设置启动子进程环境 |

稀疏身份包不应假冒 OpenAI publisher。使用自有身份还要验证上游对官方包名、sandbox service、
升级和 COM 注册的假设。官方 signed MSIX 不能简单改成 sparse 包后继续沿用原签名。
松散目录的开发注册也不能因本机恰好开启开发者模式，就宣称普通用户环境都可用。

## 接入设计与验收边界

1. 统一 Manager 的启动、安装后健康检查和重启的便携环境构造，条件性使用随包 CLI；不修改 MSIX 激活路径。
2. 发行架构匹配的原生 `LaunchCodex.exe`，桌面/开始菜单快捷方式指向启动器。保留官方 `ChatGPT.exe` 原名。
   直接双击官方 EXE 仍可能走原失败分支，不能宣传所有入口都修复。
3. 启动器相对自身位置解析文件，直接通过进程 API 传参，不把路径拼入 cmd / PowerShell。
   保留用户显式 CLI override；缺失 CLI 应明确失败，不能寻找其他产品/旧安装中的二进制来凑数。
4. 覆盖旧快捷方式迁移、更新/回滚、安装目录变更、协议回调与应用内部重启。
5. 健康检查区分“进程存活”和“主应用就绪”。原先的 3 秒存活检查可被错误弹窗骗过；
   不应把修复 launcher 的启动成功等同于应用启动成功，也不应在发行配置永久开放 CDP。
6. 对未来 payload 检测所需入口和 metadata，并做真实启动验证；不能仅硬编码版本下界。

## 尚未验证

- 登录后的真实模型对话、工具执行、浏览器扩展实际请求、完整 Computer Use 操作和 sandbox 权限。
  native pipe ready 只证明管道初始化，不能代替这些端到端测试。
- 全新机器的 GUI 启动、没有已安装 MSIX/相关服务、Windows 10 和裁剪系统。原生 ARM64 CI 已验证启动器重定位及真包模块解析，尚未验证完整 GUI 启动。
- 真实版本升级/回滚、OAuth callback、深链接、应用内部重启和系统通知。
- 测试登录页存在未登录预期的 401/授权错误；另有 Artifact Session Unix-socket 在 Windows 不可用警告。
  未对该警告做 packaged 对照，不能将其归因于此方案，也不能据此声称所有功能健康。

## 依据

- https://learn.microsoft.com/en-us/windows/win32/api/appmodel/nf-appmodel-getpackagefullname
- https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/grant-identity-to-nonpackaged-apps
- https://learn.microsoft.com/en-us/windows/apps/dev-tools/winapp-cli/guides/sparse
- https://learn.microsoft.com/en-us/windows/apps/develop/testing/loose-file-registration
- https://learn.microsoft.com/en-us/windows/msix/package/unsigned-package
