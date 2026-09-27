// Shared, language-neutral facts used by the renderer and the asset scripts.

export const ORIGIN = "https://codexapp.agentsmirror.com";
export const SKINS_ORIGIN = "https://skins.agentsmirror.com";

/** Skins rotated in the hero stage, first one is the LCP frame. */
export const HERO_SKINS = [
  "journey-to-west",
  "shinji-eva01",
  "celestial-court",
  "gundam-rx78",
  "kurumi-sunward-onmyoji",
  "western-pure-land",
];

/** Gallery lead: a varied first screen, then the rest in catalog order. */
export const FEATURED_SKINS = [
  ...HERO_SKINS,
  "ming-imperial",
  "luffy-onepiece",
  "zhang-qiling-bronze-gate",
  "asuka-eva02",
  "naraka-wanxiang-five-aspects",
  "caishen-jubao",
];

export const CATEGORY_ORDER = ["games", "anime", "guofeng", "stars", "tech"];

export const REPO = {
  mirror: "https://github.com/Wangnov/codex-app-mirror",
  manager: "https://github.com/Wangnov/Codex-App-Manager",
  skins: "https://github.com/Wangnov/awesome-codex-skins",
};

export const MANAGER_FILES = [
  { key: "mac-arm64", icon: "apple-logo", file: "CodexAppManager_aarch64.dmg" },
  { key: "mac-intel", icon: "apple-logo", file: "CodexAppManager_x86_64.dmg" },
  { key: "win-x64", icon: "windows-logo", file: "CodexAppManager_x64-setup.exe" },
  { key: "win-arm64", icon: "windows-logo", file: "CodexAppManager_arm64-setup.exe" },
].map((f) => ({ ...f, href: `${ORIGIN}/manager/latest/${f.file}` }));

export const CODEX_FILES = [
  { key: "mac-arm64", icon: "apple-logo", file: "Codex-mac-arm64.dmg", path: "/latest/mac-arm64" },
  { key: "mac-intel", icon: "apple-logo", file: "Codex-mac-x64.dmg", path: "/latest/mac-intel" },
  { key: "win-x64", icon: "windows-logo", file: "Codex-Windows-x64.msix", path: "/latest/win-x64" },
  { key: "win-arm64", icon: "windows-logo", file: "Codex-Windows-arm64.msix", path: "/latest/win-arm64" },
].map((f) => ({ ...f, href: `${ORIGIN}${f.path}` }));

export const LINKS = {
  checksums: `${ORIGIN}/latest/checksums`,
  manifest: `${ORIGIN}/latest/manifest`,
  history: `${REPO.mirror}/releases`,
  latestRelease: `${REPO.mirror}/releases/latest`,
  linux: `${REPO.mirror}/releases?q=linux-preview&expanded=true`,
  beta: `${REPO.mirror}/releases?q=codex-app-beta&expanded=true`,
  store: "https://apps.microsoft.com/detail/9plm9xgg6vks",
  signing: `${REPO.manager}/blob/main/docs/code-signing-policy.md`,
  privacy: `${REPO.manager}/blob/main/docs/privacy.md`,
  spec: `${REPO.skins}/blob/main/SPEC.md`,
  registry: `${REPO.skins}/blob/main/REGISTRY.md`,
  contributing: `${REPO.skins}#readme-cn`,
  skinPacks: `${REPO.skins}/releases/latest`,
  issues: `${REPO.mirror}/issues`,
  linuxdo: "https://linux.do/",
  duckcoding: "https://duckcoding.ai",
};

export const COMMANDS = {
  brew: "brew install --cask wangnov/tap/codex-app-manager",
  winget: "winget install Wangnov.CodexAppManager",
  skill: "npx skills add wangnov/awesome-codex-skins --skill codex-theme-maker -g",
  studio: [
    "git clone https://github.com/Wangnov/awesome-codex-skins",
    "cd awesome-codex-skins/studio",
    "node bin/codex-theme.mjs start --theme journey-to-west",
  ],
};
