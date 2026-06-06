import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

export type Lang = "zh-CN" | "en";

// Canonical copy lives here. zh-CN is the source of truth; en mirrors it.
const ZH = {
  "app.name": "Codex App 管理器",

  "nav.back": "返回",
  "nav.settings": "设置",
  "nav.about": "关于",
  "nav.uninstall": "卸载 Codex",
  "nav.config": "Codex 配置管理",

  "home.checking": "正在检查…",
  "home.idle.title": "Codex 已安装",
  "home.idle.sub": "build {build}",
  "home.uptodate.title": "已是最新",
  "home.uptodate.sub": "当前版本 {version}",
  "home.official": "官方版本",
  "home.checkedJustNow": "刚刚检查",
  "home.recheck": "重新检查",
  "home.update.title": "有新版本",
  "home.update.flow": "当前 {from} → 新版 {to}",
  "home.update.size": "约 {size}",
  "home.update.cta": "立即更新",
  "home.none.title": "未检测到 Codex",
  "home.none.sub": "为你安装最新版本,之后自动保持更新。",
  "home.none.cta": "安装 Codex",
  "home.external.title": "检测到 Codex",
  "home.external.desc": "它不是由本应用安装的。交给本应用,即可一键保持更新。",
  "home.external.cta": "开始管理",
  "home.error.title": "检查失败",
  "home.error.sub": "请稍后重试,或在设置里更换更新源。",
  "home.source": "更新源:{source}",

  "prov.managed": "已托管",
  "prov.external": "未管理",

  "confirm.title": "更新到 {version}?",
  "confirm.body": "更新时会关闭 Codex,完成后自动重启,大约一分钟。",
  "confirm.cancel": "取消",
  "confirm.ok": "更新",

  "progress.title": "正在更新…",
  "progress.installing": "正在安装…",
  "progress.downloading": "下载中 · 请稍候",

  "success.title": "已更新",
  "success.sub": "现在是 {version}",
  "success.relaunched": "Codex 已重启",
  "success.manualLaunch": "请手动启动 Codex",
  "success.rolledBack": "更新未完成,已恢复到原版本",
  "success.done": "完成",

  "settings.title": "设置",
  "settings.source.header": "更新源",
  "settings.source.auto": "自动",
  "settings.source.autoDesc": "自动选择最快、可用的来源",
  "settings.source.recommended": "推荐",
  "settings.source.mirror": "镜像",
  "settings.source.mirrorDesc": "更快、更稳定",
  "settings.source.official": "官方直连",
  "settings.source.officialDesc": "从 OpenAI 官方下载",
  "settings.source.custom": "自定义",
  "settings.source.customPlaceholder": "https://…",
  "settings.general.header": "通用",
  "settings.general.autoCheck": "自动检查更新",
  "settings.general.askBefore": "更新前询问",
  "settings.general.signedOnly": "只安装官方签名的版本",
  "settings.general.signedOnlyNote": "为你的安全,始终开启",
  "settings.appearance.header": "外观",
  "settings.appearance.theme": "主题",
  "settings.appearance.system": "跟随系统",
  "settings.appearance.light": "浅色",
  "settings.appearance.dark": "深色",
  "settings.appearance.language": "语言",
  "settings.more.header": "更多",
  "settings.more.config": "Codex 配置管理",
  "settings.more.soon": "即将支持",
  "settings.more.about": "关于",
  "settings.more.uninstall": "卸载 Codex",

  "about.tagline": "无论能否直连 OpenAI,都能保持 Codex 最新。",
  "about.version": "版本 {v}",
  "about.checkManager": "检查管理器更新",
  "about.mgrChecking": "检查中…",
  "about.feedback": "反馈与支持",

  "uninstall.heading": "卸载 Codex",
  "uninstall.warn": "这会从你的电脑上移除 Codex 应用。",
  "uninstall.needAdopt": "这是外部安装的 Codex。请先在主界面「开始管理」,再回来卸载。",
  "uninstall.keepData": "保留我的数据",
  "uninstall.keepDataNote": "登录状态、会话与配置(~/.codex)都会保留;重新安装后可继续使用。",
  "uninstall.confirm": "卸载",
  "uninstall.cancel": "取消",
  "uninstall.working": "正在卸载…",
  "uninstall.doneKept": "已卸载 Codex,并保留了你的数据。",
  "uninstall.doneCleared": "已卸载 Codex,并清除了数据。",

  "config.soon": "即将支持",
  "config.desc": "在这里管理 ~/.codex 里的会话、登录与配置。",

  "source.auto": "自动",
  "source.mirror": "镜像",
  "source.official": "官方直连",
  "source.custom": "自定义",

  "win.confirm.body": "安装时会关闭 Codex,完成后可重新打开,大约一分钟。",
  "win.route.msix": "通过 MSIX 侧载安装",
  "win.route.portable": "便携安装 · 免管理员",
  "win.installSub": "版本 {version}",
  "win.uninstall.purge": "同时清除我的数据",
  "win.uninstall.purgeNote": "登录、会话与配置都会被删除;不勾选则保留,重装后可继续使用。",
  "win.advanced.header": "Windows 高级",
  "win.autoStage.title": "自动后台暂存",
  "win.autoStage.sub": "有新版时先下载并校验,安装仍需你确认。",
  "win.autoStage.running": "正在后台暂存",
  "win.autoStage.ready": "安装包已就绪",
  "win.autoStage.skipped": "本次已跳过",
  "win.autoStage.idle": "后台暂存待命",
  "win.autoStage.waiting": "发现新版后会按网络策略自动开始。",
  "win.autoStage.cancel": "暂停",
  "win.metered.title": "允许计量网络",
  "win.metered.sub": "默认只在非计量网络自动下载。",
  "win.capabilities.header": "Windows 能力",
  "win.capabilities.sideload": "侧载策略",
  "win.capabilities.metered": "计量网络",
  "win.capabilities.recommendation": "建议路径",
  "win.capability.available": "可用",
  "win.capability.blocked": "受阻",
  "win.capability.unknown": "未知",
} as const;

type Key = keyof typeof ZH;
export type TKey = Key;

const EN: Record<Key, string> = {
  "app.name": "Codex App Manager",

  "nav.back": "Back",
  "nav.settings": "Settings",
  "nav.about": "About",
  "nav.uninstall": "Uninstall Codex",
  "nav.config": "Codex configuration",

  "home.checking": "Checking…",
  "home.idle.title": "Codex installed",
  "home.idle.sub": "build {build}",
  "home.uptodate.title": "Up to date",
  "home.uptodate.sub": "Current version {version}",
  "home.official": "Official build",
  "home.checkedJustNow": "Just checked",
  "home.recheck": "Check again",
  "home.update.title": "Update available",
  "home.update.flow": "Now {from} → New {to}",
  "home.update.size": "~{size}",
  "home.update.cta": "Update now",
  "home.none.title": "Codex not found",
  "home.none.sub": "Install the latest version and keep it updated automatically.",
  "home.none.cta": "Install Codex",
  "home.external.title": "Codex detected",
  "home.external.desc": "This Codex wasn’t installed by this app. Hand it over to keep it updated in one tap.",
  "home.external.cta": "Start managing",
  "home.error.title": "Check failed",
  "home.error.sub": "Try again later, or switch the update source in Settings.",
  "home.source": "Source: {source}",

  "prov.managed": "Managed",
  "prov.external": "Unmanaged",

  "confirm.title": "Update to {version}?",
  "confirm.body": "Codex will close and reopen automatically — about a minute.",
  "confirm.cancel": "Cancel",
  "confirm.ok": "Update",

  "progress.title": "Updating…",
  "progress.installing": "Installing…",
  "progress.downloading": "Downloading · please wait",

  "success.title": "Updated",
  "success.sub": "Now on {version}",
  "success.relaunched": "Codex restarted",
  "success.manualLaunch": "Please launch Codex manually",
  "success.rolledBack": "Update didn’t finish — restored the previous version",
  "success.done": "Done",

  "settings.title": "Settings",
  "settings.source.header": "Update source",
  "settings.source.auto": "Automatic",
  "settings.source.autoDesc": "Pick the fastest available source",
  "settings.source.recommended": "Recommended",
  "settings.source.mirror": "Mirror",
  "settings.source.mirrorDesc": "Faster and more reliable",
  "settings.source.official": "Official",
  "settings.source.officialDesc": "Download from OpenAI directly",
  "settings.source.custom": "Custom",
  "settings.source.customPlaceholder": "https://…",
  "settings.general.header": "General",
  "settings.general.autoCheck": "Check for updates automatically",
  "settings.general.askBefore": "Ask before updating",
  "settings.general.signedOnly": "Only install officially signed builds",
  "settings.general.signedOnlyNote": "Always on, for your safety",
  "settings.appearance.header": "Appearance",
  "settings.appearance.theme": "Theme",
  "settings.appearance.system": "System",
  "settings.appearance.light": "Light",
  "settings.appearance.dark": "Dark",
  "settings.appearance.language": "Language",
  "settings.more.header": "More",
  "settings.more.config": "Codex configuration",
  "settings.more.soon": "Coming soon",
  "settings.more.about": "About",
  "settings.more.uninstall": "Uninstall Codex",

  "about.tagline": "Keep Codex current — whether or not you can reach OpenAI directly.",
  "about.version": "Version {v}",
  "about.checkManager": "Check for manager updates",
  "about.mgrChecking": "Checking…",
  "about.feedback": "Feedback & support",

  "uninstall.heading": "Uninstall Codex",
  "uninstall.warn": "This removes the Codex app from your computer.",
  "uninstall.needAdopt": "This is an external Codex. Start managing it from the main screen first, then come back to uninstall.",
  "uninstall.keepData": "Keep my data",
  "uninstall.keepDataNote": "Sign-in, sessions and config (~/.codex) are kept; reinstalling resumes where you left off.",
  "uninstall.confirm": "Uninstall",
  "uninstall.cancel": "Cancel",
  "uninstall.working": "Uninstalling…",
  "uninstall.doneKept": "Codex uninstalled — your data was kept.",
  "uninstall.doneCleared": "Codex uninstalled — data cleared.",

  "config.soon": "Coming soon",
  "config.desc": "Manage the sessions, sign-in and config inside ~/.codex here.",

  "source.auto": "Automatic",
  "source.mirror": "Mirror",
  "source.official": "Official",
  "source.custom": "Custom",

  "win.confirm.body": "Codex will close during install and can reopen after — about a minute.",
  "win.route.msix": "Install via MSIX sideload",
  "win.route.portable": "Portable install · no admin",
  "win.installSub": "Version {version}",
  "win.uninstall.purge": "Also delete my data",
  "win.uninstall.purgeNote": "Sign-in, sessions and config are removed; leave it off to keep them for a reinstall.",
  "win.advanced.header": "Windows advanced",
  "win.autoStage.title": "Background pre-stage",
  "win.autoStage.sub": "Download and verify updates early; install still waits for your confirmation.",
  "win.autoStage.running": "Pre-staging in background",
  "win.autoStage.ready": "Package ready",
  "win.autoStage.skipped": "Skipped this time",
  "win.autoStage.idle": "Pre-stage idle",
  "win.autoStage.waiting": "Starts automatically when an update is found and the network policy allows it.",
  "win.autoStage.cancel": "Pause",
  "win.metered.title": "Allow metered networks",
  "win.metered.sub": "Automatic downloads default to unmetered networks only.",
  "win.capabilities.header": "Windows capabilities",
  "win.capabilities.sideload": "Sideload policy",
  "win.capabilities.metered": "Metered network",
  "win.capabilities.recommendation": "Recommended route",
  "win.capability.available": "Available",
  "win.capability.blocked": "Blocked",
  "win.capability.unknown": "Unknown",
};

const CATALOG: Record<Lang, Record<Key, string>> = { "zh-CN": ZH, en: EN };

export type TFn = (key: Key, vars?: Record<string, string | number>) => string;

interface I18nCtx {
  lang: Lang;
  setLang: (l: Lang) => void;
  t: TFn;
}

const Ctx = createContext<I18nCtx | null>(null);
const LS_KEY = "cam.lang";

function detectLang(): Lang {
  const saved = localStorage.getItem(LS_KEY);
  if (saved === "zh-CN" || saved === "en") return saved;
  return navigator.language?.toLowerCase().startsWith("zh") ? "zh-CN" : "en";
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const [lang, setLangState] = useState<Lang>(detectLang);

  const setLang = useCallback((l: Lang) => {
    setLangState(l);
    localStorage.setItem(LS_KEY, l);
    document.documentElement.lang = l;
  }, []);

  useEffect(() => {
    document.documentElement.lang = lang;
  }, [lang]);

  const t = useCallback<TFn>(
    (key, vars) => {
      let s = CATALOG[lang][key] ?? key;
      if (vars) {
        for (const [k, v] of Object.entries(vars)) {
          s = s.replace(new RegExp(`\\{${k}\\}`, "g"), String(v));
        }
      }
      return s;
    },
    [lang],
  );

  const value = useMemo(() => ({ lang, setLang, t }), [lang, setLang, t]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useI18n(): I18nCtx {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useI18n must be used within I18nProvider");
  return ctx;
}
