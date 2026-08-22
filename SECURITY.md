# 安全政策

感谢你帮助保护 Codex App Manager 及其用户。请不要在公开 Issue、Discussion 或 Pull Request 中披露尚未修复的漏洞细节。

## 受支持版本

| 版本 | 安全更新 |
| --- | --- |
| GitHub Releases 中的最新稳定版 | 支持 |
| 更早版本 | 不保证；请先升级到最新版 |

## 私密报告漏洞

请通过仓库 Security 页面中的 **Report a vulnerability** 提交私密报告：

<https://github.com/Wangnov/Codex-App-Manager/security/advisories/new>

报告中请尽量包含：

- 受影响的版本、平台与组件；
- 可复现的最小步骤或概念验证；
- 预期影响与攻击前提；
- 已知缓解措施；
- 如需署名，注明希望使用的姓名或账号。

请勿提交真实凭据、签名私钥、用户数据或会对第三方系统造成影响的利用结果。研究时请使用你有权控制的账户、设备和网络资源。

维护者的目标是在 7 天内确认收到报告，并在完成初步评估后协调修复和披露时间。复杂问题可能需要更长时间；处理期间请保持细节私密。

## 范围

本政策覆盖本仓库中的 Tauri 客户端、Rust 引擎、Cloudflare Worker、官网和 GitHub Actions。OpenAI Codex 官方应用、第三方镜像基础设施以及其他仓库中的问题，应同时报告给对应维护者；如问题由本项目的集成方式触发，仍欢迎在此私密报告。
