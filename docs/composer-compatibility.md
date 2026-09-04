# Codex 输入框素材兼容性

## 26.730.61309 结构边界

2026-09-04 对 Codex App Mirror 保存的相邻 macOS 正式版 Sparkle ZIP 进行只读提取，核对 `Info.plist`、ASAR 内的 `package.json` 和实际创建输入框表层的函数：

| Codex 版本 | build | 输入框表层 |
| --- | --- | --- |
| [26.727.51351](https://github.com/Wangnov/codex-app-mirror/releases/tag/codex-app-26.727.51351) | 6119 | 直接构造 `composer-surface-chrome`；首页通过 utility bar context 传递该类 |
| [26.730.61309](https://github.com/Wangnov/codex-app-mirror/releases/tag/codex-app-26.730.61309) | 6223 | 改用 `_ComposerLayoutRoot_uo8w3_2`，并输出 `data-composer-layout`、`data-composer-surface-variant` 等语义属性；输入框表层不再附带旧类 |
| 本机 26.901.31953 | 7868 | 同一语义属性契约，CSS-module 类为 `_ComposerLayoutRoot_kbwao_2`；已完成真实界面注入、检查和恢复 |

镜像中首次采用新结构的 macOS 正式版本是 **26.730.61309**，不是本次刚更新的 26.901 系列。
同时抽检的 26.730.61639、26.803.41515、26.810.52044、26.818.61809、26.820.80927、26.825.51511 包也已使用新结构。
这里的首次版本依据是相邻 macOS 正式包；未运行历史 Windows 安装包，不将该证据表述为 Windows 实机验证。

边界包中仍能搜索到 `composer-surface-chrome`，但它出现在静态 review card、utility bar 或浮动输入框把手中。仅统计字符串是否存在会误判，必须检查实际输入框组件的 `className` 和属性构造。

### 包校验与复核位置

按 ZIP 中央目录定位 `ChatGPT.app/Contents/Resources/app.asar` 和 `Info.plist`，范围下载压缩条目后，验证解压大小与中央目录 CRC32；再核对包内版本并计算 SHA-256。没有安装或运行历史包，也没有修改本机 Codex 安装文件。

| 版本 | ASAR 大小（字节） | ASAR CRC32（十进制） | 组件所在 chunk |
| --- | --- | --- | --- |
| 26.727.51351 | 218370611 | 2910382013 | `webview/assets/app-initial-iBPGfcXU.js` |
| 26.730.61309 | 219943419 | 1633413362 | `webview/assets/app-initial-YjNFxVhk.js` |

- 26.727.51351 ASAR SHA-256：`a529edd72e10b08931c0d695b5e3e6a0be7f51874610dafc04f578436ab7d74d`。
- 26.730.61309 ASAR SHA-256：`9de942a9a058fca20b78d171032e0fe65ccb1063868f175ff7eb4e159efc2c38`。
- 本机 26.901.31953 ASAR SHA-256：`4b385ffce845bb319a1769a3cb59751e1a8f157bab79f5018ba51d83f9e6df4e`；输入框组件位于 `webview/assets/app-primary-37ff25fd4643.js`。

## 根因与适配约束

Elon Mars Protocol 皮肤包含完整的 `composer-deck.webp`、`deck-slim.webp` 和 `send-starship.webp`。本机注入的图片数据与原始素材一致；问题不在素材缺失或 CDP 断连，而在皮肤的边框、发送按钮和图标选择器依赖旧的 `.composer-surface-chrome`。

Manager 0.5.6 从上述已确认边界的新 DOM 契约补充适配，并保留旧类路径：

- 以 `[data-composer-surface-variant][data-composer-layout]` 和实际编辑区能力定位新输入框，优先使用 Codex 的 composer 标记与根节点，不依赖每次构建都会变化的 CSS-module hash。
- 仅向有效的新输入框补上旧样式类，并标记 `data-cts-composer-surface-compat` 归属。静态 review card、无编辑区表层及嵌套外框不误加第二层装饰。
- 重复检查在 DOM 未变时不写属性；React 重建 class、任务切换或输入框重挂载后重新协调。节点失去输入框能力时撤销归属。
- 沿用编辑区、滚动通道、外壳的分层处理；仅布局/overflow 属性变化也会重新分类，避免多行输入滚动到外壳。
- 关闭皮肤或切换时清理自身新增的类与标记；旧版自带的类不占有、不删除。Rust 回退卸载路径也执行相同清理。

版本边界用于说明和回归证据，运行时仍按实际 DOM 能力判定：旧版本提示、未知新版本和不同平台不会因版本字符串而被错误拒绝；这不等于承诺兼容未来任意结构变化。
此问题与 [26.901 设置桥接接口变化](native-settings-compatibility.md) 是两个独立的兼容点。

## 验证

- 单元测试执行生产 runtime 和 Rust 卸载表达式，覆盖新旧表层、hash 变化、静态/嵌套节点排除、多输入框、重挂载、热切换、幂等与完整清理。
- 本机 Codex 26.901.31953 + Elon Mars Protocol：使用 Rust 实际打包的 payload 短暂注入后，输入框材质边框、发送按钮图片和 `LAUNCH` 装饰恢复；结构验证通过，输入框可聚焦，外壳 `overflow: clip`，编辑区 `overflow-y: auto`。随后恢复原运行时，没有发送消息或写入应用安装文件；这不是已安装新版 Manager 的端到端测试。
- 将真实当前输入框 DOM 与原生 CSS 放入隔离 Chromium，清空编辑内容后执行相同生产 payload：1100×800 深色、640×700 深色、1100×800 浅色均通过。输入测试文本、增加至 28 行、滚动、切换布局属性、重复应用和卸载均符合预期；没有相关 console 错误、框架错误遮罩或页面横向溢出。
- 历史包用于静态结构对比；未执行旧版完整应用，也未在真实 Windows Codex 中试穿皮肤。

```sh
npm test -- crates/codex-theme-engine/src/runtime
cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --locked --all-targets
```
