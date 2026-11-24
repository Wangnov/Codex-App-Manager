# Mirror & distribution service terms · 镜像与分发服务使用条款

Canonical full text / 完整权威条款原文：<https://codexapp.agentsmirror.com/terms/>

## Summary · 核心要点摘要

1. **Code vs. Hosted Service · 代码许可与在线服务分离**
   - Source code in `codex-app-mirror` and `Codex-App-Manager` is licensed under the [MIT License](../LICENSE).
   - Upstream application binaries and signatures belong to OpenAI / Microsoft and are redistributed verbatim.
   - The hosted endpoints under `*.agentsmirror.com` (including `codexapp.agentsmirror.com`, `codexapp-r2.agentsmirror.com`, and mainland China academic/open-source mirror storage supported by the **Institute of High Energy Physics, Chinese Academy of Sciences — CAS IHEP**) are **not** licensed under MIT; access is governed by the [Terms of Service](https://codexapp.agentsmirror.com/terms/).
   - 仓库源代码适用 MIT 许可证；但 `*.agentsmirror.com` 在线带宽及依托**中国科学院高能物理研究所（IHEP）**运行的国内科研与开源公益镜像存储不属于 MIT 授权范围，严格受服务条款约束。

2. **Prohibited Commercial & Abusive Use · 严禁商业寄生与滥用**
   - Strictly no integration into paid/commercial software, recharge/VIP managers, API-relay configurators, WeChat password-gated landing pages, e-commerce resale, bundled installers, reverse proxies, or User-Agent spoofing to evade access controls.
   - Forks redistributing `Codex-App-Manager` must replace default `*.agentsmirror.com` endpoints unless they qualify under the purely non-commercial open-source policy below.
   - 严禁任何收费软件、含充值/会员/代充/API 中转推广的客户端、公众号扫码密码墙、电商倒卖或反向代理调用本公益镜像；二次分发非纯公益的 Fork 必须替换默认镜像地址。

3. **Zero-Approval Open-Source Client Policy · 纯非商业开源工具免审批接入**
   - Purely free, 100% open-source tools with zero commercial/recharge/relay features may use `https://codexapp.agentsmirror.com/latest/*` without prior issue approval by displaying clear attribution, polling `/latest/manifest` no more than once every 30 minutes, verifying `SHA256SUMS`, and sending `User-Agent: <AppName>/<Version> (+https://github.com/<owner>/<repo>; open-source-noncommercial)`.
   - 完全免费开源且无任何商业变现模块的工具，遵循显著标明出处、轮询间隔 ≥30 分钟、校验哈希及标准实名 `User-Agent` 规范即可免审批直接接入。详见 <https://codexapp.agentsmirror.com/terms/#open-client-policy>。
