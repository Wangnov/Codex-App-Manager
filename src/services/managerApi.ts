import { invoke } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";

import type {
  AppSettings,
  CommandError,
  CodexUpdatePlatform,
  Diagnostics,
  MacInstallStatus,
  MacPerformReport,
  MacUninstallReport,
  MacUpdateReport,
  OperationKind,
  OperationToken,
  WinInstallStatus,
  WinPerformReport,
  WinStageReport,
  SkippedCodexUpdate,
  WinUninstallReport,
  WinUpdateReport,
} from "../shared/types";
import { DEFAULT_SETTINGS } from "../shared/types";

const SETTINGS_LS = "cam.settings";
export const SETTINGS_CHANGED_EVENT = "cam:settings-changed";
const DEFAULT_PERIODIC_CHECK_INTERVAL_SECONDS = 15 * 60;
const MIN_PERIODIC_CHECK_INTERVAL_SECONDS = 60;
const MAX_PERIODIC_CHECK_INTERVAL_SECONDS = 7 * 24 * 60 * 60;

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

interface ManagerUpdateMetadata {
  version: string;
  currentVersion: string;
  body?: string;
}

export interface FrontendErrorPayload {
  kind: string;
  message: string;
  stack: string | null;
  componentStack: string | null;
}

function normalizedInterval(value: unknown): number {
  const n = Number(value);
  if (!Number.isFinite(n)) return DEFAULT_PERIODIC_CHECK_INTERVAL_SECONDS;
  return Math.max(
    MIN_PERIODIC_CHECK_INTERVAL_SECONDS,
    Math.min(MAX_PERIODIC_CHECK_INTERVAL_SECONDS, Math.floor(n)),
  );
}

function normalizedProxyMode(value: unknown): AppSettings["proxyMode"] {
  return value === "direct" || value === "custom" || value === "system" ? value : "system";
}

function normalizedSkippedCodexUpdate(value: unknown): SkippedCodexUpdate | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as Partial<Record<keyof SkippedCodexUpdate, unknown>>;
  const platform = raw.platform;
  const target = typeof raw.target === "string" ? raw.target.trim() : "";
  const version = typeof raw.version === "string" ? raw.version.trim() : "";
  const skippedAt = Number(raw.skippedAt);
  const validPlatform = platform === "macos" || platform === "windows";
  if (!validPlatform || !target || !version || !Number.isFinite(skippedAt)) {
    return null;
  }
  return {
    platform: platform as CodexUpdatePlatform,
    target,
    version,
    skippedAt: Math.max(0, Math.floor(skippedAt)),
  };
}

function normalizeSettings(raw: Partial<AppSettings>): AppSettings {
  const legacyAuto = typeof raw.autoCheck === "boolean" ? raw.autoCheck : DEFAULT_SETTINGS.autoCheck;
  const periodic =
    typeof raw.periodicCheck === "boolean" ? raw.periodicCheck : legacyAuto;
  return {
    ...DEFAULT_SETTINGS,
    ...raw,
    autoCheck: periodic,
    checkOnStartup:
      typeof raw.checkOnStartup === "boolean" ? raw.checkOnStartup : legacyAuto,
    periodicCheck: periodic,
    periodicCheckIntervalSeconds: normalizedInterval(raw.periodicCheckIntervalSeconds),
    signedOnly: true,
    proxyMode: normalizedProxyMode(raw.proxyMode),
    customProxyUrl:
      typeof raw.customProxyUrl === "string" ? raw.customProxyUrl.trim() : "",
    disableCodexSelfUpdates:
      typeof raw.disableCodexSelfUpdates === "boolean"
        ? raw.disableCodexSelfUpdates
        : DEFAULT_SETTINGS.disableCodexSelfUpdates,
    skippedCodexUpdate: normalizedSkippedCodexUpdate(raw.skippedCodexUpdate),
  };
}

function emitSettingsChanged(settings: AppSettings) {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent<AppSettings>(SETTINGS_CHANGED_EVENT, { detail: settings }));
}

function localSettings(): AppSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_LS);
    if (raw) return normalizeSettings(JSON.parse(raw));
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

// A failing Tauri command rejects with the serialized backend `CommandError`
// ({ code, message }). Narrow defensively — `cause` may also be a JS `Error`,
// a plain string, or some other thrown value — and surface the most useful text.
function isCommandError(cause: unknown): cause is CommandError {
  if (!cause || typeof cause !== "object") {
    return false;
  }
  const maybe = cause as Partial<Record<keyof CommandError, unknown>>;
  return typeof maybe.message === "string" || typeof maybe.code === "string";
}

export function errorMessage(cause: unknown): string {
  if (cause instanceof Error) {
    return cause.message;
  }
  if (typeof cause === "string") {
    return cause;
  }
  if (isCommandError(cause)) {
    if (typeof cause.message === "string" && cause.message.trim()) {
      return cause.message;
    }
    if (typeof cause.code === "string" && cause.code.trim()) {
      return cause.code;
    }
    try {
      return JSON.stringify(cause);
    } catch {
      // fall through
    }
  }
  return String(cause);
}

/** Stable machine code from a backend `CommandError`, or null for other throwables. */
export function errorCode(cause: unknown): string | null {
  if (isCommandError(cause) && typeof cause.code === "string" && cause.code.trim()) {
    return cause.code;
  }
  return null;
}

export function isDownloadCancelled(cause: unknown): boolean {
  return errorMessage(cause).toLowerCase().includes("download cancelled");
}

// Connectivity failures (DNS / TLS / timeout / curl transport) all surface here
// as opaque engine strings. Match transport-specific text, not the generic
// "curl failed for ..." wrapper: curl exit 22 also uses that wrapper for HTTP
// 404/500 responses, where the right guidance is "try later / switch source"
// rather than "check your connection".
const NETWORK_ERROR_MARKERS = [
  "could not resolve host",
  "connection timed out",
  "operation timed out",
  "failed to connect",
  "connection reset",
  "connection refused",
  "network is unreachable",
  "empty reply from server",
  "appcast are unreachable",
  "curl: (6)",
  "curl: (7)",
  "curl: (28)",
  "curl: (35)",
  "curl: (52)",
  "curl: (56)",
  "ssl/tls",
  "schannel",
];

/** Heuristic: does this surfaced error message describe a connectivity failure
 *  rather than a logic/verification error? Lets the UI show a calm "can't reach
 *  the update server, retry" instead of a raw curl diagnostic. */
export function isNetworkError(message: string): boolean {
  const m = message.toLowerCase();
  return NETWORK_ERROR_MARKERS.some((marker) => m.includes(marker));
}

// Browser-dev fallbacks (no Tauri runtime) so the UI renders meaningfully
// outside the desktop shell. macOS simulates an update; Windows simulates the
// up-to-date metadata view.
const FALLBACK_PLAN: MacUpdateReport = {
  appcastUrl: "https://persistent.oaistatic.com/codex-app-prod/appcast.xml",
  installed: null,
  simulatedBuild: 3511,
  latestPubDate: "Fri, 26 Jun 2026 10:10:00 GMT",
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

const WIN_FALLBACK_INSTALLED = {
  path: "C:\\Program Files\\WindowsApps\\OpenAI.Codex_26.623.4041.0_x64__2p2nqsd0c76g0",
  version: "26.623.4041.0",
  arch: "x64",
  source: "msix",
  packageFamilyName: "OpenAI.Codex_2p2nqsd0c76g0",
  installedAt: 1_782_489_000,
};

const WIN_FALLBACK_PLAN: WinUpdateReport = {
  manifestUrl: "https://codexapp.agentsmirror.com/latest/manifest",
  checksumsUrl: "https://codexapp.agentsmirror.com/latest/checksums",
  packageUrl: "https://codexapp.agentsmirror.com/latest/win",
  release: {
    version: "26.623.4041.0",
    releasedAt: "Fri, 26 Jun 2026 10:10:00 GMT",
    packageMoniker: "OpenAI.Codex_26.623.4041.0_x64__2p2nqsd0c76g0",
    architecture: "x64",
    contentLength: 566504666,
    etag: '"4XJflSyTxVc59Sr2FfA4HAqFCD0="',
    storeProductId: "9PLM9XGG6VKS",
    packageIdentity: "OpenAI.Codex",
  },
  installed: WIN_FALLBACK_INSTALLED,
  capabilities: {
    addAppxPackage: { state: "available", detail: "browser-dev mock" },
    appxService: { state: "available", detail: "browser-dev mock" },
    sideloadPolicy: { state: "unknown", detail: "browser-dev mock" },
    appInstaller: { state: "available", detail: "browser-dev mock" },
    msixDeployment: { state: "available", detail: "browser-dev mock" },
    meteredNetwork: { state: "unknown", detail: "browser-dev mock" },
    recommendation: "msix-preferred",
    notes: ["Certificate trust is verified after the MSIX is staged."],
  },
  plan: {
    upToDate: true,
    currentVersion: "26.623.4041.0",
    latestVersion: "26.623.4041.0",
    packageMoniker: "OpenAI.Codex_26.623.4041.0_x64__2p2nqsd0c76g0",
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
  packageMoniker: "OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0",
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
  msixHealth: { healthy: true, verified: true, packageRegistered: true, status: "Ok", statusOk: true, aumidResolved: true, missingDependencies: [], reason: "" },
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
    partial: false,
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

const FALLBACK_DIAGNOSTICS: Diagnostics = {
  appVersion: "0.0.0",
  os: "browser",
  arch: "unknown",
  locale: null,
  updateSource: "auto",
  customSourceHost: null,
  windowsInstallMode: null,
  installStatus: "browser preview",
  configHealth: {
    settingsStatus: "ok",
    provenanceStatus: "ok",
    unknownSource: null,
    detail: null,
  },
  logsDir: null,
  recentErrors: [],
  logTail: "",
  generatedAtUnix: Math.floor(Date.now() / 1000),
};

export const managerApi = {
  armDestructive(kind: OperationKind): Promise<OperationToken> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(`browser-dev-token-${kind}`);
    }
    return invoke<OperationToken>("arm_destructive", { kind });
  },
  macPlanUpdate(simulatedBuild?: number): Promise<MacUpdateReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ ...FALLBACK_PLAN, simulatedBuild: simulatedBuild ?? null });
    }
    return invoke<MacUpdateReport>("mac_plan_update", {
      simulatedBuild: simulatedBuild ?? null,
    });
  },
  // Destructive: reconstruct + codesign-gate + atomic swap + relaunch (or
  // rollback). `confirm` must be true; the backend rejects it otherwise. Guarded
  // by a UI second confirmation before this is ever called. The expected target
  // (from/to build + install path the user confirmed) is sent so the backend can
  // refuse if reality drifted (appcast refresh / Codex self-update) since confirm.
  async macPerformUpdate(expected: {
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
        relaunchFailed: false,
        rolledBack: false,
        warning: null,
        message: "（浏览器开发态：真实替换仅在桌面 app 内执行）",
      });
    }
    const token = await managerApi.armDestructive("update");
    return invoke<MacPerformReport>("mac_perform_update", {
      confirm: true,
      token,
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
    const update = await invoke<ManagerUpdateMetadata | null>("manager_check_update").catch(
      () => undefined,
    );
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
  macPauseDownload(): Promise<boolean> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(true);
    }
    return invoke<boolean>("mac_pause_download");
  },
  macCancelDownload(): Promise<boolean> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(true);
    }
    return invoke<boolean>("mac_cancel_download");
  },
  // Discard a PAUSED download's cached partial (paused-state cancel).
  macDiscardDownload(): Promise<void> {
    if (!hasTauriRuntime()) {
      return Promise.resolve();
    }
    return invoke<void>("mac_discard_download");
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
  getDiagnostics(): Promise<Diagnostics> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({
        ...FALLBACK_DIAGNOSTICS,
        generatedAtUnix: Math.floor(Date.now() / 1000),
      });
    }
    return invoke<Diagnostics>("get_diagnostics");
  },
  openLogsDir(): Promise<void> {
    if (!hasTauriRuntime()) {
      return Promise.resolve();
    }
    return invoke<void>("open_logs_dir");
  },
  openCodexHome(): Promise<void> {
    if (!hasTauriRuntime()) {
      return Promise.resolve();
    }
    return invoke<void>("open_codex_home");
  },
  reportFrontendError(payload: FrontendErrorPayload): Promise<void> {
    if (!hasTauriRuntime()) {
      console.error("[frontend]", payload);
      return Promise.resolve();
    }
    return invoke<void>("log_frontend_error", { payload }).catch((cause) => {
      console.error("[frontend]", payload, cause);
    });
  },

  // Settings (update source + general). The backend persists them so the source
  // choice actually drives which appcast the update flow reads.
  async getSettings(): Promise<AppSettings> {
    if (!hasTauriRuntime()) {
      return localSettings();
    }
    try {
      return normalizeSettings(await invoke<AppSettings>("get_settings"));
    } catch {
      return localSettings();
    }
  },
  async setSettings(next: AppSettings): Promise<AppSettings> {
    const safe = normalizeSettings(next);
    if (!hasTauriRuntime()) {
      localStorage.setItem(SETTINGS_LS, JSON.stringify(safe));
      emitSettingsChanged(safe);
      return safe;
    }
    const saved = normalizeSettings(await invoke<AppSettings>("set_settings", { settings: safe }));
    localStorage.setItem(SETTINGS_LS, JSON.stringify(saved));
    emitSettingsChanged(saved);
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
      emitSettingsChanged(saved);
      return saved;
    }
    const saved = normalizeSettings(await invoke<AppSettings>("win_set_install_root", { path }));
    localStorage.setItem(SETTINGS_LS, JSON.stringify(saved));
    emitSettingsChanged(saved);
    return saved;
  },
  async winResetInstallRoot(): Promise<AppSettings> {
    if (!hasTauriRuntime()) {
      const saved = { ...localSettings(), installRoot: DEFAULT_SETTINGS.installRoot };
      localStorage.setItem(SETTINGS_LS, JSON.stringify(saved));
      emitSettingsChanged(saved);
      return saved;
    }
    const saved = normalizeSettings(await invoke<AppSettings>("win_reset_install_root"));
    localStorage.setItem(SETTINGS_LS, JSON.stringify(saved));
    emitSettingsChanged(saved);
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
  async macUninstall(keepCodexHome: boolean): Promise<MacUninstallReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({
        removed: true,
        keptCodexHome: keepCodexHome,
        message: keepCodexHome ? "（浏览器开发态）已卸载,保留数据" : "（浏览器开发态）已卸载并清除数据",
      });
    }
    const token = await managerApi.armDestructive("uninstall");
    return invoke<MacUninstallReport>("mac_uninstall", { confirm: true, token, keepCodexHome });
  },
  winPlanUpdate(): Promise<WinUpdateReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(WIN_FALLBACK_PLAN);
    }
    return invoke<WinUpdateReport>("win_plan_update");
  },
  winPauseDownload(): Promise<boolean> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(true);
    }
    return invoke<boolean>("win_pause_download");
  },
  winCancelDownload(): Promise<boolean> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(true);
    }
    return invoke<boolean>("win_cancel_download");
  },
  // Discard a PAUSED download's cached partial (paused-state cancel).
  winDiscardDownload(): Promise<void> {
    if (!hasTauriRuntime()) {
      return Promise.resolve();
    }
    return invoke<void>("win_discard_download");
  },
  async winPerformUpdate(
    confirm: boolean,
    expected?: {
      currentVersion: string | null;
      latestVersion: string;
      packageMoniker: string;
      route: string;
    },
    installRoot?: string,
  ): Promise<WinPerformReport> {
    if (!hasTauriRuntime()) {
      return confirm
        ? Promise.resolve(WIN_FALLBACK_PERFORM)
        : Promise.reject(new Error("explicit confirmation is required"));
    }
    if (!confirm) {
      return Promise.reject(new Error("explicit confirmation is required"));
    }
    const token = await managerApi.armDestructive("update");
    return invoke<WinPerformReport>("win_perform_update", {
      confirm,
      token,
      installRoot: installRoot ?? null,
      expected: expected ?? null,
    });
  },
  async winUninstall(confirm: boolean, purgeUserData: boolean): Promise<WinUninstallReport> {
    if (!hasTauriRuntime()) {
      return confirm
        ? Promise.resolve({ ...WIN_FALLBACK_UNINSTALL, purgedUserData: purgeUserData })
        : Promise.reject(new Error("explicit confirmation is required"));
    }
    if (!confirm) {
      return Promise.reject(new Error("explicit confirmation is required"));
    }
    const token = await managerApi.armDestructive("uninstall");
    return invoke<WinUninstallReport>("win_uninstall", { confirm, token, purgeUserData });
  },
  winStatus(): Promise<WinInstallStatus> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ installed: WIN_FALLBACK_INSTALLED, status: "managed" });
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

function managerUpdateAvailable(update: ManagerUpdateMetadata): ManagerUpdateAvailable {
  return {
    kind: "available",
    version: update.version,
    currentVersion: update.currentVersion,
    body: update.body,
    installAndRelaunch: async () => {
      await invoke<void>("manager_install_update", {
        expectedVersion: update.version,
        expectedCurrentVersion: update.currentVersion,
      });
      await relaunch();
    },
    discard: async () => {},
  };
}
