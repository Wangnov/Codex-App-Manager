import { invoke } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";

import type {
  AppSettings,
  MacInstallStatus,
  MacPerformReport,
  MacStageReport,
  MacUninstallReport,
  MacUpdateReport,
  WinAutoStageReport,
  WinInstallStatus,
  WinPerformReport,
  WinStageReport,
  WinUninstallReport,
  WinUpdateReport,
} from "../shared/types";
import { DEFAULT_SETTINGS } from "../shared/types";

const SETTINGS_LS = "cam.settings";

export type ManagerUpdateCheck =
  | { kind: "development" }
  | { kind: "unavailable" }
  | { kind: "none" }
  | ManagerUpdateAvailable;

export interface ManagerUpdateAvailable {
  kind: "available";
  version: string;
  currentVersion: string;
  body?: string;
  installAndRelaunch: () => Promise<void>;
  discard: () => Promise<void>;
}

function localSettings(): AppSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_LS);
    if (raw) return { ...DEFAULT_SETTINGS, ...JSON.parse(raw), signedOnly: true };
  } catch {
    // ignore
  }
  return { ...DEFAULT_SETTINGS };
}

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

function hasTauriRuntime(): boolean {
  return typeof window !== "undefined" && Boolean(window.__TAURI_INTERNALS__);
}

export function errorMessage(cause: unknown): string {
  if (cause instanceof Error) {
    return cause.message;
  }
  if (typeof cause === "string") {
    return cause;
  }
  if (cause && typeof cause === "object") {
    const maybe = cause as { message?: unknown; code?: unknown };
    if (typeof maybe.message === "string" && maybe.message.trim()) {
      return maybe.message;
    }
    if (typeof maybe.code === "string" && maybe.code.trim()) {
      return maybe.code;
    }
    try {
      return JSON.stringify(cause);
    } catch {
      // fall through
    }
  }
  return String(cause);
}

// Browser-dev fallbacks (no Tauri runtime) — a simulated "one version behind"
// so the UI renders meaningfully outside the desktop shell.
const FALLBACK_PLAN: MacUpdateReport = {
  appcastUrl: "https://persistent.oaistatic.com/codex-app-prod/appcast.xml",
  installed: null,
  simulatedBuild: 3511,
  plan: {
    upToDate: false,
    currentBuild: 3511,
    latestBuild: 3575,
    latestShortVersion: "26.602.30954",
    strategy: { kind: "delta", fromBuild: 3511 },
    downloadUrl:
      "https://persistent.oaistatic.com/codex-app-prod/Codex3575-3511-arm64.delta",
    downloadSize: 18260894,
    edSignature: "(browser-dev mock)",
    fullSize: 406581087,
    savingsPct: 95.5,
  },
};

const FALLBACK_STAGE: MacStageReport = {
  upToDate: false,
  strategy: "delta-from-3511",
  latestBuild: 3575,
  latestShortVersion: "26.602.30954",
  downloadSize: 18260894,
  fullSize: 406581087,
  savingsPct: 95.5,
  stagedPath: "(browser-dev mock) …/Codex3575-3511-arm64.delta",
  verified: true,
};

const WIN_FALLBACK_PLAN: WinUpdateReport = {
  manifestUrl: "https://codexapp.agentsmirror.com/latest/manifest",
  checksumsUrl: "https://codexapp.agentsmirror.com/latest/checksums",
  packageUrl: "https://codexapp.agentsmirror.com/latest/win",
  release: {
    version: "26.602.3474.0",
    packageMoniker: "OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0",
    architecture: "x64",
    contentLength: 566504666,
    etag: '"4XJflSyTxVc59Sr2FfA4HAqFCD0="',
    storeProductId: "9PLM9XGG6VKS",
    packageIdentity: "OpenAI.Codex",
  },
  installed: null,
  capabilities: {
    addAppxPackage: { state: "available", detail: "browser-dev mock" },
    appxService: { state: "available", detail: "browser-dev mock" },
    sideloadPolicy: { state: "unknown", detail: "browser-dev mock" },
    appInstaller: { state: "available", detail: "browser-dev mock" },
    meteredNetwork: { state: "unknown", detail: "browser-dev mock" },
    recommendation: "msix-preferred",
    notes: ["Certificate trust is verified after the MSIX is staged."],
  },
  plan: {
    upToDate: false,
    currentVersion: null,
    latestVersion: "26.602.3474.0",
    packageMoniker: "OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0",
    packageUrl: "https://codexapp.agentsmirror.com/latest/win",
    downloadSize: 566504666,
    sha256: "6dc2e05ac2b760bbc77ce3f8a992efdb327363512c9c4744b9a146c41bc4d55a",
    route: "msix-sideload",
    portableFallbackReady: true,
    warnings: [],
  },
};

const WIN_FALLBACK_STAGE: WinStageReport = {
  upToDate: false,
  route: "msix-sideload",
  latestVersion: "26.602.3474.0",
  downloadSize: 566504666,
  stagedPath: "(browser-dev mock) …/OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0.msix",
  sha256: "6dc2e05ac2b760bbc77ce3f8a992efdb327363512c9c4744b9a146c41bc4d55a",
  hashVerified: true,
  authenticode: {
    trusted: true,
    publisherIsOpenai: true,
    status: "Valid",
    statusMessage: "browser-dev mock",
    subject: "CN=OpenAI OpCo, LLC",
    issuer: "browser-dev mock",
    thumbprint: "browser-dev mock",
  },
  identity: {
    name: "OpenAI.Codex",
    publisher: "CN=OpenAI OpCo, LLC",
    version: "26.602.3474.0",
    processorArchitecture: "x64",
  },
  identityVerified: true,
  installReady: true,
  portableFallbackReady: true,
  notes: ["Non-destructive staging only; install execution is the next guarded step."],
};

const WIN_FALLBACK_AUTO_STAGE: WinAutoStageReport = {
  enabled: true,
  allowMetered: false,
  attempted: true,
  skipped: false,
  reason: "staged",
  stage: WIN_FALLBACK_STAGE,
  capabilities: WIN_FALLBACK_PLAN.capabilities,
  notes: ["browser-dev mock: package staged in the background."],
};

const WIN_FALLBACK_PERFORM: WinPerformReport = {
  success: true,
  action: "msix-sideload",
  message: "browser-dev mock: Add-AppxPackage succeeded",
  stage: WIN_FALLBACK_STAGE,
  sideload: {
    success: true,
    message: "browser-dev mock: Add-AppxPackage succeeded",
    installed: {
      path: "C:\\Program Files\\WindowsApps\\OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0",
      version: "26.602.3474.0",
      arch: "x64",
      source: "msix",
      packageFamilyName: "OpenAI.Codex_2p2nqsd0c76g0",
    },
    fallbackRecommended: false,
    rawError: null,
  },
  portable: null,
  msixHealth: { healthy: true, packageRegistered: true, status: "Ok", statusOk: true, aumidResolved: true, missingDependencies: [], reason: "" },
  installed: {
    path: "C:\\Program Files\\WindowsApps\\OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0",
    version: "26.602.3474.0",
    arch: "x64",
    source: "msix",
    packageFamilyName: "OpenAI.Codex_2p2nqsd0c76g0",
  },
  fallbackAvailable: true,
  fallbackAttempted: false,
  notes: ["browser-dev mock: install path is simulated."],
};

const WIN_FALLBACK_UNINSTALL: WinUninstallReport = {
  success: true,
  action: "remove-portable",
  message: "browser-dev mock: uninstall completed",
  installedBefore: WIN_FALLBACK_PERFORM.installed,
  msix: null,
  portable: {
    success: true,
    installRoot: "%LOCALAPPDATA%\\Programs\\Codex",
    removedFiles: true,
    removedShortcut: true,
    removedUninstallEntry: true,
    purgedUserData: false,
    message: "browser-dev mock: portable uninstall completed",
    notes: ["browser-dev mock"],
  },
  purgedUserData: false,
  notes: ["User data was preserved."],
};

export const managerApi = {
  macPlanUpdate(simulatedBuild?: number): Promise<MacUpdateReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ ...FALLBACK_PLAN, simulatedBuild: simulatedBuild ?? null });
    }
    return invoke<MacUpdateReport>("mac_plan_update", {
      simulatedBuild: simulatedBuild ?? null,
    });
  },
  macStageUpdate(simulatedBuild?: number): Promise<MacStageReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(FALLBACK_STAGE);
    }
    return invoke<MacStageReport>("mac_stage_update", {
      simulatedBuild: simulatedBuild ?? null,
    });
  },
  // Destructive: reconstruct + codesign-gate + atomic swap + relaunch (or
  // rollback). `confirm` must be true; the backend rejects it otherwise. Guarded
  // by a UI second confirmation before this is ever called. The expected target
  // (from/to build + install path the user confirmed) is sent so the backend can
  // refuse if reality drifted (appcast refresh / Codex self-update) since confirm.
  macPerformUpdate(expected: {
    fromBuild: number;
    toBuild: number;
    path: string;
  }): Promise<MacPerformReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({
        upToDate: false,
        fromBuild: expected.fromBuild,
        toBuild: expected.toBuild,
        strategy: "delta-from-3511",
        installedPath: expected.path,
        verified: true,
        relaunched: true,
        rolledBack: false,
        message: "（浏览器开发态：真实替换仅在桌面 app 内执行）",
      });
    }
    return invoke<MacPerformReport>("mac_perform_update", {
      confirm: true,
      expectedFromBuild: expected.fromBuild,
      expectedToBuild: expected.toBuild,
      expectedPath: expected.path,
    });
  },
  // Self-update the manager itself via the Tauri updater (minisign-signed,
  // full bundle). Endpoints + signing are server-side (see roadmap §4).
  async checkManagerUpdate(): Promise<ManagerUpdateCheck> {
    if (!hasTauriRuntime()) {
      return { kind: "development" };
    }
    // A routine check shouldn't surface a scary error when the release feed
    // isn't published yet or is unreachable.
    const update = await check().catch(() => undefined);
    if (update === undefined) {
      return { kind: "unavailable" };
    }
    if (!update) {
      return { kind: "none" };
    }
    return managerUpdateAvailable(update);
  },
  macStatus(): Promise<MacInstallStatus> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ installed: null, status: "none" });
    }
    return invoke<MacInstallStatus>("mac_status");
  },
  macAdopt(): Promise<MacInstallStatus> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ installed: null, status: "managed" });
    }
    return invoke<MacInstallStatus>("mac_adopt");
  },

  // Fresh-install the latest Codex (full package) into /Applications.
  macInstall(): Promise<MacInstallStatus> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({
        installed: {
          path: "/Applications/Codex.app",
          build: 3575,
          shortVersion: "26.602.30954",
          arch: "arm64",
        },
        status: "managed",
      });
    }
    return invoke<MacInstallStatus>("mac_install");
  },
  // Open the installed Codex — explicit user action after install (we no longer
  // auto-launch).
  macLaunch(): Promise<void> {
    if (!hasTauriRuntime()) {
      return Promise.resolve();
    }
    return invoke<void>("mac_launch_codex");
  },
  // Open an external URL in the system browser (a webview <a target=_blank> is a
  // no-op under Tauri).
  openUrl(url: string): Promise<void> {
    if (!hasTauriRuntime()) {
      window.open(url, "_blank");
      return Promise.resolve();
    }
    return invoke<void>("open_url", { url });
  },

  // Settings (update source + general). The backend persists them so the source
  // choice actually drives which appcast the update flow reads.
  async getSettings(): Promise<AppSettings> {
    if (!hasTauriRuntime()) {
      return localSettings();
    }
    try {
      return await invoke<AppSettings>("get_settings");
    } catch {
      return localSettings();
    }
  },
  async setSettings(next: AppSettings): Promise<AppSettings> {
    const safe = { ...next, signedOnly: true };
    if (!hasTauriRuntime()) {
      localStorage.setItem(SETTINGS_LS, JSON.stringify(safe));
      return safe;
    }
    const saved = await invoke<AppSettings>("set_settings", { settings: safe });
    localStorage.setItem(SETTINGS_LS, JSON.stringify(saved));
    return saved;
  },
  // The user confirmed the close dialog — tell the backend to actually exit
  // (the window/exit guards otherwise hold the close to ask first).
  confirmQuit(): Promise<void> {
    if (!hasTauriRuntime()) {
      return Promise.resolve();
    }
    return invoke<void>("confirm_quit");
  },
  winPickInstallDir(): Promise<string | null> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(localSettings().installRoot);
    }
    return invoke<string | null>("win_pick_install_dir");
  },
  winDefaultInstallRoot(): Promise<string> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(DEFAULT_SETTINGS.installRoot);
    }
    return invoke<string>("win_default_install_root");
  },
  async winSetInstallRoot(path: string): Promise<AppSettings> {
    if (!hasTauriRuntime()) {
      const saved = { ...localSettings(), installRoot: path };
      localStorage.setItem(SETTINGS_LS, JSON.stringify(saved));
      return saved;
    }
    const saved = await invoke<AppSettings>("win_set_install_root", { path });
    localStorage.setItem(SETTINGS_LS, JSON.stringify(saved));
    return saved;
  },
  async winResetInstallRoot(): Promise<AppSettings> {
    if (!hasTauriRuntime()) {
      const saved = { ...localSettings(), installRoot: DEFAULT_SETTINGS.installRoot };
      localStorage.setItem(SETTINGS_LS, JSON.stringify(saved));
      return saved;
    }
    const saved = await invoke<AppSettings>("win_reset_install_root");
    localStorage.setItem(SETTINGS_LS, JSON.stringify(saved));
    return saved;
  },

  // Launch-at-login (off by default). Backed by tauri-plugin-autostart.
  getAutostart(): Promise<boolean> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(false);
    }
    return invoke<boolean>("get_autostart");
  },
  setAutostart(enabled: boolean): Promise<void> {
    if (!hasTauriRuntime()) {
      return Promise.resolve();
    }
    return invoke<void>("set_autostart", { enabled });
  },

  // Destructive: remove the Codex app. keepCodexHome defaults to true so the
  // user's ~/.codex (sign-in, sessions, config) survives unless they opt out.
  macUninstall(keepCodexHome: boolean): Promise<MacUninstallReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({
        removed: true,
        keptCodexHome: keepCodexHome,
        message: keepCodexHome ? "（浏览器开发态）已卸载,保留数据" : "（浏览器开发态）已卸载并清除数据",
      });
    }
    return invoke<MacUninstallReport>("mac_uninstall", { confirm: true, keepCodexHome });
  },
  winPlanUpdate(): Promise<WinUpdateReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(WIN_FALLBACK_PLAN);
    }
    return invoke<WinUpdateReport>("win_plan_update");
  },
  winStageUpdate(): Promise<WinStageReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(WIN_FALLBACK_STAGE);
    }
    return invoke<WinStageReport>("win_stage_update");
  },
  winAutoStageUpdate(enabled: boolean, allowMetered: boolean): Promise<WinAutoStageReport> {
    if (!hasTauriRuntime()) {
      if (!enabled) {
        return Promise.resolve({
          ...WIN_FALLBACK_AUTO_STAGE,
          enabled,
          allowMetered,
          attempted: false,
          skipped: true,
          reason: "disabled",
          stage: null,
          notes: ["browser-dev mock: automatic pre-download is disabled."],
        });
      }
      return Promise.resolve({ ...WIN_FALLBACK_AUTO_STAGE, enabled, allowMetered });
    }
    return invoke<WinAutoStageReport>("win_auto_stage_update", { enabled, allowMetered });
  },
  winCancelDownload(): Promise<boolean> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(false);
    }
    return invoke<boolean>("win_cancel_download");
  },
  winPerformUpdate(confirm: boolean, installRoot?: string): Promise<WinPerformReport> {
    if (!hasTauriRuntime()) {
      return confirm
        ? Promise.resolve(WIN_FALLBACK_PERFORM)
        : Promise.reject(new Error("explicit confirmation is required"));
    }
    return invoke<WinPerformReport>("win_perform_update", {
      confirm,
      installRoot: installRoot ?? null,
    });
  },
  winUninstall(confirm: boolean, purgeUserData: boolean): Promise<WinUninstallReport> {
    if (!hasTauriRuntime()) {
      return confirm
        ? Promise.resolve({ ...WIN_FALLBACK_UNINSTALL, purgedUserData: purgeUserData })
        : Promise.reject(new Error("explicit confirmation is required"));
    }
    return invoke<WinUninstallReport>("win_uninstall", { confirm, purgeUserData });
  },
  winStatus(): Promise<WinInstallStatus> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ installed: null, status: "none" });
    }
    return invoke<WinInstallStatus>("win_status");
  },
  winAdopt(): Promise<WinInstallStatus> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ installed: WIN_FALLBACK_PLAN.installed, status: "managed" });
    }
    return invoke<WinInstallStatus>("win_adopt");
  },
  // Open the installed Codex — explicit user action (mirrors macLaunch).
  winLaunch(): Promise<void> {
    if (!hasTauriRuntime()) {
      return Promise.resolve();
    }
    return invoke<void>("win_launch_codex");
  },
};

function managerUpdateAvailable(update: Update): ManagerUpdateAvailable {
  return {
    kind: "available",
    version: update.version,
    currentVersion: update.currentVersion,
    body: update.body,
    installAndRelaunch: async () => {
      await update.downloadAndInstall();
      await relaunch();
    },
    discard: async () => {
      await update.close().catch(() => undefined);
    },
  };
}
