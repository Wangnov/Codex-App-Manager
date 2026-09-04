# Codex 热切换设置接口兼容性

## 26.901 原生桥接边界

2026-09-04 对镜像中的相邻 macOS 正式包与本机安装包进行了对比：

| Codex 版本 | 设置包装函数 | 调用结构 |
| --- | --- | --- |
| [26.831.21537](https://github.com/Wangnov/codex-app-mirror/releases/tag/codex-app-26.831.21537) | `QD` / `$D` | 直接调用 `get-setting` / `set-setting` RPC |
| [26.901.20858](https://github.com/Wangnov/codex-app-mirror/releases/tag/codex-app-26.901.20858)（build 7658） | `xO` / `SO` | 优先使用 app-host 的 `settings.read` / `settings.write`，缺席时回退 RPC |
| [26.901.22334](https://github.com/Wangnov/codex-app-mirror/releases/tag/codex-app-26.901.22334)（build 7746） | `bO` / `xO` | 同上；本机真实读写验证版本 |

镜像中首次出现该结构的 macOS 正式版本是 **26.901.20858**，不是 26.901.22334。
从该版本起优先使用新适配器。版本只决定探测顺序；旧版本提示或未知版本仍会探测其他适配器，并验证实际函数结构。
本次未运行历史 Windows 安装包，不将 macOS 包的证据写成 Windows 实机验证。

26.901.20858 的 Sparkle ZIP 中 `ChatGPT.app/Contents/Resources/app.asar` 经 ZIP 大小和 CRC32 校验：

- 大小：296097431 字节。
- SHA-256：`3dee62fa9bfabc58d1c96d9de1b04ab2a09597e78b9f39f14403e26d627f8b73`。
- 设置所在 chunk：`webview/assets/app-initial-7a6c8787453d.js`。
- 本机 26.901.22334 对应 chunk：`webview/assets/app-initial-f1c3ba37268a.js`。

## 根因与修复约束

Manager 0.5.3 的正则要求读写函数的整个函数体都是旧 RPC 形状。新版本增加原生桥接分支后，CDP 仍正常连通，但三个旧适配器均无法定位设置模块。

新适配器导入候选 chunk 后检查实际导出函数，按参数与 RPC 回退载荷识别读写契约，不依赖压缩函数名、导出别名或完整函数体形状。读写必须唯一且使用同一个 RPC 调用入口；同一函数的重复导出别名会去重；无法检查的无关导出会跳过。Codex 的函数监测包装可能使 `Function.prototype.toString` 抛错，这一情况已在本机实测并纳入回归测试。

定位期间不调用设置读写函数。调用时仍执行 Codex 自己的包装函数，保留原生桥接选择、RPC 回退、默认值与副作用。旧版 26.707、26.715、26.831.21537 适配器继续保留；未知新结构若无法证明匹配，仍会明确报错，不声称兼容所有未来版本。

## 验证

- `src/runtime/native-hot-discovery.test.mjs`（相对 `crates/codex-theme-engine/`）执行 Rust 中嵌入的生产脚本与真实 ESM 导入，覆盖新旧包装函数、原生/RPC 两条路径、切换及恢复、缓存、懒加载、过期版本提示、歧义/错误参数拒绝与无法检查的导出。
- Rust 单元测试固定 `26.901.20858` 边界与完整探测顺序。
- 本机 26.901.22334：旧版定位脚本仍复现 `settings module not found`；修复后的 `live_hot_settings_same_value_round_trip` 完成五项外观设置读取、原值写回和复读一致性验证，不改变当前视觉设置。

```sh
npm test -- crates/codex-theme-engine/src/runtime/native-hot-discovery.test.mjs
cargo test --manifest-path crates/codex-theme-engine/Cargo.toml native_hot --lib
# 仅在开发机已运行启用 9345 CDP 端口的 Codex 时执行；会写回现有外观设置。
cargo test --manifest-path crates/codex-theme-engine/Cargo.toml \
  --test real_packages live_hot_settings_same_value_round_trip -- --ignored --nocapture
```
