export default {
  meta: {
    title: "Codex App Manager — 官方 Codex 桌面应用的安装、更新与卸载管家",
    description:
      "一键安装、增量更新、干净卸载官方 OpenAI Codex 桌面应用。macOS Sparkle 增量更新、EdDSA 逐字节校验、R2 + IHEP 双镜像,国内免代理直连。Tauri v2 构建,MIT 开源。",
    ogTitle: "Codex App Manager — 一键装好官方 Codex,自动保持最新",
    ogDescription:
      "原样镜像官方安装包,送达你的 Mac 与 PC,每一步都可校验。macOS 增量更新、失败自动回滚、国内直连可达。macOS 版本已通过 Developer ID 签名和 Apple 公证,项目开源可审计。",
  },
  nav: {
    why: "为什么",
    manager: "管理器",
    skins: "皮肤",
    pipeline: "镜像链路",
    trust: "可信验证",
    download: "下载",
    scope: "边界声明",
    cta: "下载",
  },
  skins: {
    kicker: "主题皮肤",
    title: "给 Codex 换一身素材化 UI",
    sub: "在线皮肤库一键安装,运行中的 Codex 秒级试穿、随时还原——不修改任何应用文件,签名完好。",
    altGuts: "TPC GUTS 指挥终端皮肤(迪迦奥特曼)",
    altRei: "NERV 零号机 绫波丽皮肤",
    points: [
      {
        title: "真机截图,真机质量门",
        body: "每张预览都是注入后运行中 Codex 的真实截图,经自动化验收产出——没有效果图。",
      },
      {
        title: "完全可逆",
        body: "皮肤经 CDP 注入,不改 app.asar、不破坏签名;关闭即刻回到原生外观。",
      },
      {
        title: "开放标准与生产线",
        body: ".codexskin 是公开标准;用仓库里的 Agent Skill(Claude Code / Codex 通用),几小时做出你自己的皮肤。",
      },
    ],
    cta: "浏览 awesome-codex-skins 皮肤画廊 →",
  },
  hero: {
    eyebrow: "开源项目 · macOS 与 Windows",
    titleA: "官方 Codex 桌面应用",
    titleB: "一键装好,持续最新",
    tagline: "一键安装、增量更新、干净卸载官方 Codex 桌面应用,自带国内可达的自更新。",
    sub: "Windows 不用 Microsoft Store,macOS 只下载版本之间的增量。R2 与 IHEP 双镜像,国内直连可达,无需代理。",
    ctaPrimary: "立即下载",
    ctaSecondary: "GitHub 仓库",
    allOptions: "全部下载方式",
    dl: {
      macArm: "下载 macOS 版(Apple Silicon)",
      macIntel: "下载 macOS 版(Intel)",
      win: "下载 Windows 版(x64)",
    },
    scrollHint: "往下看,它是怎么做到的",
    badges: [
      "macOS Developer ID 签名 + Apple 公证",
      "MIT 开源",
      "Tauri v2",
      "11 种语言",
    ],
  },
  demo: {
    window: "Codex App Manager",
    updTitle: "有新版本",
    updVer: "26.602.71036",
    updFlow: "当前 26.602.40724 → 新版 26.602.71036 · 约 12.6 MiB",
    updCta: "立即更新",
    launch: "启动 Codex",
    recheck: "重新检查",
    checking: "正在检查…",
    progressTitle: "正在更新…",
    progressFrom: "正在从 镜像 下载",
    uptodateTitle: "已是最新",
    uptodateSub: "当前版本 26.602.71036",
    official: "官方版本 · 刚刚检查",
    successFlow: "已更新 26.602.40724 → 26.602.71036",
    releaseDate: "发布时间",
    releaseVal: "今天",
    loc: "安装位置",
    locVal: "/Applications/Codex.app",
    source: "更新源:镜像",
    settings: "设置",
    managed: "已托管",
  },
  pain: {
    kicker: "为什么",
    title: "装个官方应用,不该这么难",
    lead: "Codex 桌面应用本身很好。难的是把它顺利装上、按时更新、干净卸掉——尤其当你身在国内。",
    cards: [
      {
        title: "Microsoft Store,指望不上",
        body: "官方 Windows 版只在 Microsoft Store 分发。商店连不上、账号登录不了、设备被策略禁用——任何一条,都让你装不上 Codex。",
      },
      {
        title: "下载与更新,都压在海外线路上",
        body: "macOS 官方直链和 Sparkle 更新链路都托管在海外。网络一抖,下载重试、更新停在半路,版本越落越远。",
      },
      {
        title: "手动安装,留下一地痕迹",
        body: "自己下载、覆盖、删除,旧版本残留、状态错乱。出了问题没有回滚,只能从头再来。",
      },
    ],
    mock: {
      file: "Codex_Installer.dmg",
      host: "来源:海外节点",
      status: "网络超时,正在重试…",
      retry: "第 3 次重试",
    },
  },
  manager: {
    kicker: "Codex App Manager",
    title: "安装、更新、卸载,三步完成",
    lead: "Manager 不急着改动你的系统。它先检测本地的 Codex 安装状态,再生成一份计划,最后才谨慎执行——破坏性操作之前,逐项核验。",
    steps: [
      {
        name: "检测",
        title: "看清本地的一切",
        body: "识别本机 Codex 的安装状态、版本与残留。这一步不碰任何文件,只是看清楚。",
      },
      {
        name: "规划",
        title: "把每一步写在明处",
        body: "根据检测结果生成执行计划:装什么、换什么、删什么。先告诉你,再开始。",
      },
      {
        name: "执行",
        title: "谨慎地完成",
        body: "破坏性操作前先验证,执行后确认结果;macOS 上更新校验失败会自动回滚。装好之后,一键启动 Codex。",
      },
    ],
    scenarios: [
      {
        label: "安装",
        desc: "一次点击,从零到可用。装完即可直接拉起 Codex。",
      },
      {
        label: "更新",
        desc: "macOS 上消费 Sparkle appcast,只下载版本之间的增量,EdDSA 签名逐字节校验。",
      },
      {
        label: "回滚",
        desc: "更新失败自动回到上一个可用版本,不留半成品在你的系统里。",
      },
    ],
    extras: [
      {
        title: "一键启动",
        body: "安装、更新之后不必去翻启动台。在 Manager 里直接打开 Codex。",
      },
      {
        title: "Windows 无需 Microsoft Store",
        body: "直接安装官方 MSIX 或便携版,更新分阶段进行,装完自动执行健康检查——商店连不上也照样用。",
      },
      {
        title: "11 种语言,一套温和的界面",
        body: "OKLCH 暖色材质,深浅两套主题,GSAP 动效,支持包括阿拉伯语 RTL 在内的 11 种语言。",
      },
    ],
    mock: {
      window: "Codex App Manager",
      detect: {
        scan: "正在检测本机环境…",
        found: "发现 Codex 桌面应用",
        ver: "版本落后于上游",
        leftover: "检测到旧版残留",
      },
      plan: {
        title: "执行计划",
        i1: "下载增量更新包",
        i2: "校验 EdDSA 签名",
        i3: "替换应用文件",
        i4: "运行健康检查",
        note: "破坏性操作前将逐项核验",
      },
      exec: {
        doing: "正在应用更新…",
        ok: "更新完成 · 签名校验通过",
        launch: "启动 Codex",
        rollback: "如校验失败,将自动回滚",
      },
    },
  },
  pipeline: {
    kicker: "Codex App Mirror",
    title: "从官方上游,到你的桌面",
    lead: "Mirror 负责把官方安装包原样送到离你最近的节点,每一步都可验证;Manager 把这份能力变成桌面上的安装与更新体验。下面是一个安装包的完整链路。",
    stages: [
      {
        title: "官方上游",
        body: "一切始于 OpenAI 官方发布的安装包。我们不生产字节,只负责送达。",
        stat: "MSIX + DMG",
      },
      {
        title: "每 15 分钟看一眼",
        body: "Cloudflare Cron 持续探测官方上游,GitHub Actions 作为后备。上游一有变化,即刻知晓。",
        stat: "15 min",
      },
      {
        title: "原样自动发布",
        body: "Windows MSIX 与 macOS DMG(arm64 + x64)逐字节镜像,零修改、零重新打包。每个版本附带 SHA256SUMS 与上游指纹清单。",
        stat: "SHA256",
      },
      {
        title: "双镜像落地",
        body: "R2 面向全球,IHEP S3 面向中国大陆。同一份字节,落在两个节点。",
        stat: "R2 + IHEP",
      },
      {
        title: "一条短链,就近抵达",
        body: "Cloudflare Worker 按 CF-IPCountry 路由:大陆走 IHEP 预签名链接,其余走 R2。你只需要记住一个地址。",
        stat: "CF-IPCountry",
      },
      {
        title: "增量更新,落到桌面",
        body: "macOS 上,Manager 读取 Sparkle appcast,只取版本之间的差异。官方 EdDSA 签名逐字节校验,失败自动回滚。",
        stat: "EdDSA",
      },
    ],
    nodes: ["官方上游", "15 分钟探测", "自动发版", "双镜像", "地域分流", "你的桌面"],
    branchGlobal: "全球 → R2",
    branchCN: "中国大陆 → IHEP S3",
    verifiedChip: "EdDSA 校验通过",
    finale: "从官方上游到你的 Mac 与 PC,每一个字节都原样、可验证。",
  },
  trust: {
    kicker: "可信验证",
    title: "每一环,都可验证",
    lead: "不需要相信我们的说法。镜像与官方是否一致,任何人随时都能自己验证。",
    mock: {
      title: "校验对比",
      tag: "示意",
      official: "官方 SHA256",
      mirror: "镜像 SHA256",
      sig: "EdDSA 签名",
      sigVal: "逐字节复制官方签名",
      match: "完全一致",
    },
    items: [
      {
        title: "Developer ID 签名与公证",
        body: "macOS 版本由 Apple Developer ID 签名,并通过 Apple 公证。来源可查,系统原生验证。",
      },
      {
        title: "EdDSA 逐字节校验",
        body: "镜像逐字节复制官方 EdDSA 签名——签名无法伪造,我们也从不伪造。校验不通过,更新就不会安装。",
      },
      {
        title: "SHA256SUMS 与上游指纹",
        body: "每个镜像版本附带 SHA256SUMS 与上游指纹清单。任何人,任何时候,都能比对镜像与官方是否一致。",
      },
      {
        title: "开源,可审计",
        body: "Manager 与 Mirror 全部以 MIT 协议开源。每一行代码、每一条流水线,都摆在明处供你审阅。",
      },
    ],
  },
  download: {
    kicker: "下载",
    title: "下载 Codex App Manager",
    lead: "选择适合你的方式。所有直链都是镜像永久链接,始终指向最新版本,国内直连可达。",
    recommended: "为你推荐",
    brew: {
      title: "Homebrew",
      note: "macOS 推荐",
    },
    direct: [
      {
        platform: "macOS · Apple Silicon",
        label: "下载 .dmg",
        note: "永久链接,始终最新 · 国内直连可达",
      },
      {
        platform: "macOS · Intel",
        label: "下载 .dmg",
        note: "永久链接,始终最新 · 国内直连可达",
      },
      {
        platform: "Windows · x64",
        label: "下载 .exe",
        note: "永久链接,始终最新 · 无需 Microsoft Store",
      },
      {
        platform: "Windows · ARM64",
        label: "下载 .exe",
        note: "永久链接,始终最新 · Windows on Arm 原生",
      },
    ],
    github: {
      title: "GitHub",
      note: "代码、Issue 与每一次发布记录,都公开在这里。",
      managerLabel: "Wangnov/Codex-App-Manager",
      mirrorLabel: "Wangnov/codex-app-mirror",
    },
    signing: {
      status: "Windows Authenticode 的 SignPath Foundation 申请仍在审核;当前 Windows 安装器未签名。",
      attributionNote: "若申请获批并完成独立接入,项目将按要求标注:",
      policy: "Code signing policy · 代码签名政策",
      privacy: "隐私政策",
    },
  },
  scope: {
    kicker: "边界声明",
    title: "做什么,不做什么",
    items: [
      {
        title: "原样分发,从不改包",
        body: "不修改、不重新打包官方安装程序。EdDSA 签名逐字节复制——它无法被伪造,我们也绝不伪造。",
      },
      {
        title: "独立项目,无任何隶属",
        body: "本项目是独立的社区工具,与 OpenAI、Microsoft 均无隶属或背书关系。",
      },
      {
        title: "MIT 开源",
        body: "Manager 与 Mirror 均以 MIT 协议开源,代码与构建流水线公开,可审计。",
      },
    ],
    mit: "MIT License — 自由使用、修改与分发。",
  },
  footer: {
    thanks: "感谢中国科学院高能物理研究所(IHEP)为中国大陆提供镜像节点。",
    license: "MIT License · 开源可审计",
    made: "用 Tauri,和一点耐心做成。",
    backTop: "回到顶部",
    links: {
      manager: "Codex App Manager",
      mirror: "Codex App Mirror",
      signingPolicy: "Code signing policy · 代码签名政策",
      privacyPolicy: "隐私政策",
    },
  },
  ui: {
    langSwitch: "EN",
    langAria: "切换语言",
    menu: "菜单",
    close: "关闭",
    skip: "跳到主要内容",
    copy: "复制命令",
    copied: "已复制",
  },
} as const;
