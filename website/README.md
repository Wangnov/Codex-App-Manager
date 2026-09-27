# Codex App 全家桶官网

<https://codexapp.agentsmirror.com> 的源码。一个站点介绍三个开源项目：

- [codex-app-mirror](https://github.com/Wangnov/codex-app-mirror)：官方 Codex 桌面版安装包镜像
- [Codex App Manager](https://github.com/Wangnov/Codex-App-Manager)：安装、增量更新、卸载与换肤的桌面管理器
- [awesome-codex-skins](https://github.com/Wangnov/awesome-codex-skins)：`.codexskin` 标准、皮肤画廊与制作流水线

## 结构

| 路径 | 作用 |
|---|---|
| `site/render.mjs` | 页面模板。构建期把 `index.html`（中文，`/`）、`en/index.html`（英文，`/en/`）、`404.html` 渲染成完整静态 HTML |
| `site/locales/{zh,en}.mjs` | 文案唯一来源，可含少量受信任的内联 HTML |
| `site/config.mjs` | 首屏轮播皮肤、下载直链、仓库链接、安装命令等与语言无关的事实 |
| `site/data/site.json` | 构建期数据快照（镜像最新版本与哈希、Manager 版本、Star 数、下载量、皮肤目录） |
| `site/manifest.mjs` | 镜像 `release-manifest.json` 的摘要逻辑，构建脚本与 Worker 共用 |
| `src/main.ts`、`src/styles/` | 浏览器端增强：导航、实时数据、平台识别、首屏换肤、画廊筛选与弹窗、下载分页、复制按钮 |
| `worker/index.js` | 站点 Worker，只处理 `/api/*`；`/api/status.json` 从 R2 读取最新版本信息 |
| `public/_headers` | 安全头与缓存策略，构建后由 `scripts/finalize-headers.mjs` 写入内联脚本哈希 |

页面在没有 JavaScript 时也完整可读；实时数据拿不到时保留构建期快照。

## 开发

```bash
npm install
npm run dev        # Vite dev server，改 site/ 下文件会整页刷新
npm run build      # 产出 dist/，并把内联脚本哈希写进 dist/_headers
```

## 数据与素材

```bash
npm run data       # 刷新 site/data/site.json，下载新的皮肤预览到 assets/raw/skins/
npm run images     # assets/raw → public/img（AVIF/WebP 多尺寸、皮肤缩略图、Manager 截图）
npm run fonts      # 字体子集化 → public/fonts
node scripts/readme-banner.mjs   # 用官网素材重新生成仓库 README 横幅 assets/banner.svg
```

`assets/raw/` 与 `assets/fonts-src/` 不进仓库：

- `bg-*.png`、`obj-*.png`：用 gpt-image-2-skill 的 Codex provider（`--provider codex --model gpt-6-astra`）生成的云海、镜湖背景与光泽 3D 物件；透明物件走 `transparent generate`（绿幕抠像）并通过 `transparent verify --strict` 验收。
- `skins/*.webp`：`npm run data` 从 skins.agentsmirror.com 下载的真机截图。
- `manager/*.png`：在 Manager 仓库根目录 `npm run dev` 打开浏览器预览模式，用真实发布数据截取的界面（深浅色 × 中英文）。
- `logo-mirror.png`：codex-app-mirror 仓库的 `assets/logo.png`。
- `fonts-src/SourceHanSansCN-{Heavy,Bold}.otf`：思源黑体（OFL）。Mona Sans 与 Monaspace Neon 来自 Fontsource 开发依赖。

改了文案后重跑 `npm run fonts`：中文标题字体按实际用字子集化，缺字会回退到系统字体。

## 部署

```bash
npm run deploy     # = npm run build && npx wrangler@4 deploy
```

zone 路由按最长匹配分流，互不抢路：

- `/manager/*` → 本仓库 `cloudflare/manager-download-router`
- `/latest/*`、`/stats/*` → codex-app-mirror 的 download-router
- `/*` → 本站（静态资源优先，只有 `/api/*` 进入 Worker；未知路径返回 `404.html`）

Worker 绑定了 `codex-app-mirror` 与 `codex-app-manager` 两个 R2 桶，只做读取，响应在边缘缓存 5 分钟。
