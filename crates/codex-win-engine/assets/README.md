# Windows 便携启动器图标

`launcher.ico` 使用官方 Codex Windows 安装包内的 `app/resources/icon-chatgpt.ico`，与管理器创建 Codex 快捷方式时优先选择的官方图标一致。

它只嵌入 Manager 自己构建的便携启动器，不修改官方可执行文件或 ASAR。不要从 `src-tauri/icons/icon.ico` 复制图标：那是 Codex App Manager 自身的图标，嵌入启动器会使 Codex 的入口和任务栏固定项显示成管理器图标（#473）。

2026-10-29 提取来源：

- 下载入口：<https://codexapp.agentsmirror.com/latest/win-x64>（此入口会随最新版变化，核对下方固定包身份与哈希）。
- 包身份：`OpenAI.Codex_26.1023.6621.0_x64__2p2nqsd0c76g0`。
- Codex 应用版本：`26.1023.51421`。
- MSIX SHA256：`fda8b07433d2e8872b8833bb665552eb8de6475ba9cca54fb8af28436629d133`，已与镜像清单中 Microsoft Store catalog 的 SHA256 校验一致。
- 包内路径：`app/resources/icon-chatgpt.ico`。
- ICO SHA256：`56b60f54c33d204eb6c0bfea1283fc3e82c0d3966d428fe98176a2335d2fe57c`，保留官方 ICO 的全部 8 个图像条目。

更新时先校验完整来源包，再按原样提取该文件。`build.rs` 会为 x64 和 ARM64 启动器编译同一份图标资源；现有启动器在安装或启动修复路径上按内嵌字节更新，保持幂等写入。
