# Issue #353：MSIX 运行时启动失败和健康检查误判

调查日期：2026-09-23。Issue：https://github.com/Wangnov/Codex-App-Manager/issues/353

## 已确认的问题

最新报告使用 Manager 0.5.8、MSIX `OpenAI.Codex_26.917.6896.0_x64__2p2nqsd0c76g0`，
应用版本 `26.917.51856`。错误是 `Unable to locate the Codex CLI binary or required runtime components`。
它不同于最初报告的“该进程没有程序包标识符”。诊断中较早的 `0x80073D02` 和便携目录访问拒绝，
不能解释后一次已成功完成的 MSIX 安装。

0.5.8 的 MSIX 探针只要求进程存活三秒，启动错误弹窗也满足这个条件，因而可能输出
`healthy=true status=Ok`，跳过已有的便携降级。手动 MSIX 启动也只检查激活请求是否提交。

## 修复和恢复路径

- MSIX 安装验证和手动启动都按实际注册目录查找进程，再检查主窗口和原生错误弹窗。
  主窗口出现后须连续稳定三秒，整个窗口等待最长三十秒。错误标题或正文优先于主窗口。
- 进程查询和窗口读取使用原生 Win32；每次窗口检查总计有界，单个跨进程文本读取最多 50 ms。
  正常探针只关闭自己安装目录内的 Codex/ChatGPT GUI，不把包内后台服务误判为清理失败。
- MSIX 探针失败会进入现有便携安装事务：从同一份已校验的 MSIX 部署便携 payload，
  通过启动验证后才移除失效的 MSIX。便携启动器指定随包 CLI，避开 WindowsApps 运行时定位路径。
  便携验证失败则保留错误并走原有回滚，不宣称安装成功。
- 已经安装 0.5.8 的用户可在安装方式中选便携版，再选择同一版本重新安装修复。
  使用此修复的 Manager，可保持 MSIX 方式并重新安装同一版本，让验证决定是否降级。
  修复后的便携版也可通过 `LaunchCodex.exe` / 开始菜单启动，无须常驻 Manager。
- 失败前保存必要的应用级运行时线索；复制诊断增加随包组件可读性、Manager 环境覆盖是否存在，
  以及最近 Codex 日志中的已知启动事件。只保留事件名和白名单枚举字段，不复制原始应用日志、
  环境变量值、命令、会话或认证数据。目录遍历、文件数和单文件读取量均有限制。

## 真实版本验证

从 issue 对应的镜像 release 获取完整 x64 MSIX，其 SHA256 为：

`5c9bfe6ab03ad2bf7a70bb86022c889f74b67c8b9cc871fa14569773420bbb70`

该值与 `SHA256SUMS-windows.txt` 一致；引擎验证官方 Authenticode 后按正式提取逻辑创建独立 payload。
使用独立 Codex home 和 Electron profile，便携启动通过了新的主窗口稳定性检查。
2026-09-23T07:24:48Z 的独立实例日志确认：

- `windows_core_runtime_component_resolved`：CLI 指向实验 payload 的 `resources/codex.exe`，
  `source=override`、`appPackageCoreActive=false`、`copiedRuntimeLocation=false`。
- app-server 版本 `0.155.0-alpha.16`，`initialized=true`、`next=connected`、`transport=stdio`。
- 主窗口已经显示；测试后关闭全部实验进程，清除实验 Chrome host discovery 条目并恢复既有 host manifest。

这验证了该版本的便携恢复路径，不等于在报告者机器上复现并确定其 MSIX 内部根因，也不代表登录后全部功能已测试。

## 尚待报告者环境证据

静态检查该版本官方 ASAR，通用运行时错误来自 CLI/Node 定位返回空结果，可能经过不同路径：
应用包运行时定位、WindowsApps 可执行文件迁移缓存失败，或启用 WSL 时发行版解析失败。
完整发布包中存在 CLI、command runner、sandbox helper、Node 和 node_repl；不能据此证明报告者的
已安装文件和本地运行时缓存都正常。无效 CLI override 也不必然触发该错误，上游部分分支会回退。

后续应依据新增诊断中的 `bundled_executable_relocation_failed`、运行时来源、组件可读性和 WSL 事件
进一步修复具体原因；不自动删除用户缓存、不重置 WSL 设置、不写全局 CLI 覆盖。
在报告者验证恢复前，issue 保持打开。

## 回归验证

- Windows 原生子进程窗口夹具覆盖：正常主窗口、仅错误标题、仅错误正文、主窗口出现后再报错，
  以及其他安装目录的窗口不应被接纳。
- 单元测试覆盖稳定期、失败优先级、清理语义、错误报告分类和诊断字段脱敏。
- `real_msix_portable_startup` 是需要官方包和带 `.codex-manager-smoke` 标记的独立目录的可选实包测试；
  执行后保留实验目录，便于查日志。应用自身可能更新共享 Chrome host discovery，测试操作者需在结束后恢复实验条目。
