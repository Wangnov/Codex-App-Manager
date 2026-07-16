import { invoke } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";

import type {
  AncillaryRetryReport,
  AncillaryRetryRequest,
  AppSettings,
  CodexThemeStatusReport,
  CodexThemeSummary,
  CommandError,
  CodexUpdatePlatform,
  ConfigHealth,
  ConfigWhich,
  Diagnostics,
  MacInstallStatus,
  MacPerformReport,
  MacUninstallReport,
  MacUpdateReport,
  OperationKind,
  OperationSnapshot,
  OperationCompletion,
  OperationToken,
  WinInstallStatus,
  WinPerformReport,
  WinStageReport,
  SkippedCodexUpdate,
  WindowMode,
  WindowModeReport,
  WinUninstallReport,
  WinUpdateReport,
} from "../shared/types";
import { DEFAULT_SETTINGS, emptyOperationOutcome } from "../shared/types";

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
  const customUrl = typeof raw.customUrl === "string" ? raw.customUrl.trim() : "";
  const customProxyUrl =
    typeof raw.customProxyUrl === "string" ? raw.customProxyUrl.trim() : "";
  // Empty custom modes are not a real runtime choice — fall back so UI, disk,
  // and update paths agree (mirror/auto for source, system proxy for network).
  let source = raw.source ?? DEFAULT_SETTINGS.source;
  if (source === "custom" && !customUrl) {
    source = "auto";
  }
  let proxyMode = normalizedProxyMode(raw.proxyMode);
  if (proxyMode === "custom" && !customProxyUrl) {
    proxyMode = "system";
  }
  return {
    ...DEFAULT_SETTINGS,
    ...raw,
    source,
    customUrl,
    autoCheck: periodic,
    checkOnStartup:
      typeof raw.checkOnStartup === "boolean" ? raw.checkOnStartup : legacyAuto,
    periodicCheck: periodic,
    periodicCheckIntervalSeconds: normalizedInterval(raw.periodicCheckIntervalSeconds),
    signedOnly: true,
    proxyMode,
    customProxyUrl,
    disableCodexSelfUpdates:
      typeof raw.disableCodexSelfUpdates === "boolean"
        ? raw.disableCodexSelfUpdates
        : DEFAULT_SETTINGS.disableCodexSelfUpdates,
    skippedCodexUpdate: normalizedSkippedCodexUpdate(raw.skippedCodexUpdate),
    codexTheme:
      typeof raw.codexTheme === "string" && raw.codexTheme.trim() ? raw.codexTheme : null,
    codexThemeDir:
      typeof raw.codexThemeDir === "string" && raw.codexThemeDir.trim()
        ? raw.codexThemeDir
        : null,
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
  path: "C:\\Program Files\\WindowsApps\\OpenAI.Codex_26.623.5546.0_x64__2p2nqsd0c76g0",
  version: "26.623.42026",
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
    version: "26.623.42026",
    packageVersion: "26.623.5546.0",
    releasedAt: "Sat, 27 Jun 2026 05:28:48 GMT",
    packageMoniker: "OpenAI.Codex_26.623.5546.0_x64__2p2nqsd0c76g0",
    architecture: "x64",
    contentLength: 671037642,
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
    currentVersion: "26.623.42026",
    latestVersion: "26.623.42026",
    packageMoniker: "OpenAI.Codex_26.623.5546.0_x64__2p2nqsd0c76g0",
    packageUrl: "https://codexapp.agentsmirror.com/latest/win",
    downloadSize: 671037642,
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
  msixHealth: { healthy: true, verified: true, packageRegistered: true, status: "Ok", statusOk: true, aumidResolved: true, missingDependencies: [], activationOk: true, failureKind: "", reason: "" },
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
  outcome: emptyOperationOutcome({
    primaryOk: true,
    appState: "present",
    installClass: "managed",
  }),
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
  outcome: emptyOperationOutcome({
    primaryOk: true,
    appState: "absent",
    installClass: "none",
  }),
};

const FALLBACK_CONFIG_HEALTH: ConfigHealth = {
  settingsStatus: "ok",
  provenanceStatus: "ok",
  unknownSource: null,
  detail: null,
  settingsBackupAvailable: false,
  provenanceBackupAvailable: false,
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
  configHealth: { ...FALLBACK_CONFIG_HEALTH },
  logsDir: null,
  recentErrors: [],
  logTail: "",
  generatedAtUnix: Math.floor(Date.now() / 1000),
};

// Browser-preview stand-ins so the theme gallery can be developed and styled
// without a Tauri backend (mirrors guts-terminal / asuka-eva02 palettes).
const BROWSER_FALLBACK_THEMES: CodexThemeSummary[] = [
  {
    id: "guts-terminal",
    name: "GUTS Terminal",
    description: "TPC/GUTS command terminal from the Tiga concept sheets.",
    dir: "/dev/themes/guts-terminal",
    hasNativeTheme: true,
    colors: {
      base: "#1a1d24",
      panel: "#232833",
      accent: "#d97e2a",
      ink: "#f2e9d8",
      glow: "#e8a33d",
      line: "#3a4150",
    },
  },
  {
    id: "asuka-eva02",
    name: "Asuka EVA-02",
    description: "NERV console styling around Unit-02's palette.",
    dir: "/dev/themes/asuka-eva02",
    hasNativeTheme: true,
    colors: {
      base: "#17131a",
      panel: "#241c26",
      accent: "#ff6a00",
      ink: "#f5ede4",
      glow: "#c8300e",
      line: "#453343",
    },
  },
];

const BROWSER_FALLBACK_THEME_STATUS: CodexThemeStatusReport = {
  supported: true,
  activeTheme: null,
  daemon: null,
  cdpReady: false,
  codexRunning: false,
  nativeBackupPresent: false,
};

// ── Contract guards ──────────────────────────────────────────────────────────
// invoke<T>() is a bare type assertion: if the backend contract drifts (field
// renamed, made nullable), the UI would otherwise crash deep in a render with
// an unhelpful stack. Guard the load-bearing read paths — the report/status
// shapes every home-screen decision keys on — and fail them as a readable
// CommandError instead. Only structural keystones are checked, not every
// field: enough to catch a desynced engine, cheap enough to never drift far
// from the real types.

function contractError(what: string): CommandError {
  return {
    code: "contract_mismatch",
    message: `Backend returned an unexpected ${what} shape — the app and its engine may be out of sync. Reinstalling the manager usually fixes this.`,
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object";
}

function guardMacStatus(status: MacInstallStatus): MacInstallStatus {
  const s = status as unknown;
  if (
    !isRecord(s) ||
    typeof s.status !== "string" ||
    (s.installed !== null && !(isRecord(s.installed) && typeof s.installed.build === "number"))
  ) {
    throw contractError("macOS install status");
  }
  return status;
}

function guardMacReport(report: MacUpdateReport): MacUpdateReport {
  const r = report as unknown;
  if (
    !isRecord(r) ||
    (r.installed !== null && !(isRecord(r.installed) && typeof r.installed.build === "number")) ||
    (r.plan !== null && !(isRecord(r.plan) && typeof r.plan.upToDate === "boolean"))
  ) {
    throw contractError("macOS update report");
  }
  return report;
}

function guardWinStatus(status: WinInstallStatus): WinInstallStatus {
  const s = status as unknown;
  if (
    !isRecord(s) ||
    typeof s.status !== "string" ||
    (s.installed !== null && !(isRecord(s.installed) && typeof s.installed.version === "string"))
  ) {
    throw contractError("Windows install status");
  }
  return status;
}

function guardWinReport(report: WinUpdateReport): WinUpdateReport {
  const r = report as unknown;
  if (
    !isRecord(r) ||
    !(isRecord(r.plan) && typeof r.plan.upToDate === "boolean") ||
    (r.installed !== null && !(isRecord(r.installed) && typeof r.installed.version === "string"))
  ) {
    throw contractError("Windows update report");
  }
  return report;
}

export const managerApi = {
  armDestructive(kind: OperationKind): Promise<OperationToken> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(`browser-dev-token-${kind}`);
    }
    return invoke<OperationToken>("arm_destructive", { kind });
  },
  /** Active install/update lease, or null when idle. Queried on mount to reattach. */
  getOperationSnapshot(): Promise<OperationSnapshot | null> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(null);
    }
    return invoke<OperationSnapshot | null>("get_operation_snapshot");
  },
  /** Token-keyed terminal evidence retained after a renderer loses its invoke. */
  getOperationCompletion(token: OperationToken): Promise<OperationCompletion | null> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(null);
    }
    return invoke<OperationCompletion | null>("get_operation_completion", { token });
  },
  macPlanUpdate(simulatedBuild?: number): Promise<MacUpdateReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ ...FALLBACK_PLAN, simulatedBuild: simulatedBuild ?? null });
    }
    return invoke<MacUpdateReport>("mac_plan_update", {
      simulatedBuild: simulatedBuild ?? null,
    }).then(guardMacReport);
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
    return invoke<MacInstallStatus>("mac_status").then(guardMacStatus);
  },
  macAdopt(): Promise<MacInstallStatus> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ installed: null, status: "managed" });
    }
    return invoke<MacInstallStatus>("mac_adopt");
  },
  macPickExistingInstall(): Promise<MacInstallStatus["installed"] | null> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({
        path: "/Users/example/Desktop/Codex.app",
        build: 4041,
        shortVersion: "26.623.31443",
        arch: "arm64",
      });
    }
    return invoke<MacInstallStatus["installed"] | null>("mac_pick_existing_install");
  },
  macAdoptPath(path: string): Promise<MacInstallStatus> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({
        installed: {
          path,
          build: 4041,
          shortVersion: "26.623.31443",
          arch: "arm64",
        },
        status: "managed",
      });
    }
    return invoke<MacInstallStatus>("mac_adopt_path", { path });
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
  macPauseDownload(operationId: string): Promise<boolean> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(true);
    }
    return invoke<boolean>("mac_pause_download", { operationId });
  },
  macCancelDownload(operationId: string): Promise<boolean> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(true);
    }
    return invoke<boolean>("mac_cancel_download", { operationId });
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
  frontendReady(lang: string, generation: number, token: string): Promise<void> {
    if (!hasTauriRuntime()) {
      return Promise.resolve();
    }
    return invoke<void>("frontend_ready", { lang, generation, token });
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
  // ── Codex UI themes ────────────────────────────────────────────────────
  /** Locally installed theme packages (managed dir + optional dev dir). */
  codexThemeList(): Promise<CodexThemeSummary[]> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(BROWSER_FALLBACK_THEMES);
    }
    return invoke<CodexThemeSummary[]>("codex_theme_list");
  },
  codexThemeStatus(): Promise<CodexThemeStatusReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ ...BROWSER_FALLBACK_THEME_STATUS });
    }
    return invoke<CodexThemeStatusReport>("codex_theme_status");
  },
  /** Live try-on against an already-debuggable Codex. Not persisted. */
  codexThemeTryOn(themeRef: string): Promise<CodexThemeStatusReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ ...BROWSER_FALLBACK_THEME_STATUS, daemon: null });
    }
    return invoke<CodexThemeStatusReport>("codex_theme_try_on", { themeRef });
  },
  /** Persist the current try-on as the standing selection. */
  codexThemeKeep(themeRef: string): Promise<void> {
    if (!hasTauriRuntime()) {
      return Promise.resolve();
    }
    return invoke<void>("codex_theme_keep", { themeRef });
  },
  /** Full apply: restart Codex debuggable + native config sections + inject. */
  codexThemeApply(themeRef: string): Promise<CodexThemeStatusReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ ...BROWSER_FALLBACK_THEME_STATUS, activeTheme: themeRef });
    }
    return invoke<CodexThemeStatusReport>("codex_theme_apply", { themeRef });
  },
  /** Turn the theme off; `full` also restores original config.toml sections. */
  codexThemeOff(full: boolean): Promise<CodexThemeStatusReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ ...BROWSER_FALLBACK_THEME_STATUS, activeTheme: null });
    }
    return invoke<CodexThemeStatusReport>("codex_theme_off", { full });
  },
  /** Switch the native window between compact and expanded. `size` is the
   *  remembered expanded size (logical px); the report echoes what was applied
   *  after work-area clamping. The browser preview has no native window, so it
   *  echoes the request and the layout switches on CSS alone. */
  setWindowMode(
    mode: WindowMode,
    size?: { width: number; height: number },
  ): Promise<WindowModeReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({
        mode,
        width: size?.width ?? (mode === "expanded" ? 1100 : 400),
        height: size?.height ?? (mode === "expanded" ? 720 : 640),
      });
    }
    return invoke<WindowModeReport>("set_window_mode", {
      mode,
      width: size?.width,
      height: size?.height,
    });
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
        outcome: emptyOperationOutcome({
          primaryOk: true,
          appState: "absent",
          installClass: "none",
          cleanup: {
            state: keepCodexHome ? "skipped" : "ok",
            detail: keepCodexHome ? "kept user data" : null,
          },
        }),
      });
    }
    const token = await managerApi.armDestructive("uninstall");
    return invoke<MacUninstallReport>("mac_uninstall", { confirm: true, token, keepCodexHome });
  },
  getConfigHealth(): Promise<ConfigHealth> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ ...FALLBACK_CONFIG_HEALTH });
    }
    return invoke<ConfigHealth>("get_config_health");
  },
  restoreConfigBackup(which: ConfigWhich): Promise<ConfigHealth> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ ...FALLBACK_CONFIG_HEALTH });
    }
    return invoke<ConfigHealth>("restore_config_backup", { which });
  },
  resetConfig(which: ConfigWhich): Promise<ConfigHealth> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ ...FALLBACK_CONFIG_HEALTH });
    }
    return invoke<ConfigHealth>("reset_config", { which });
  },
  async retryAncillary(request: AncillaryRetryRequest): Promise<AncillaryRetryReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({
        message: "browser-dev mock: ancillary retry ok",
        outcome: emptyOperationOutcome({
          primaryOk: true,
          appState: "unknown",
          installClass: null,
        }),
      });
    }
    // purge_user_data is destructive — arm the same uninstall token as a full
    // uninstall so recovery cannot one-click wipe ~/.codex.
    const wantsPurge =
      Boolean(request.purgeUserData) &&
      request.actions.includes("purge_user_data");
    if (wantsPurge) {
      const token = await managerApi.armDestructive("uninstall");
      return invoke<AncillaryRetryReport>("retry_ancillary", {
        request,
        confirm: true,
        token,
      });
    }
    return invoke<AncillaryRetryReport>("retry_ancillary", {
      request,
      confirm: null,
      token: null,
    });
  },
  winPlanUpdate(): Promise<WinUpdateReport> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(WIN_FALLBACK_PLAN);
    }
    return invoke<WinUpdateReport>("win_plan_update").then(guardWinReport);
  },
  winPauseDownload(operationId: string): Promise<boolean> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(true);
    }
    return invoke<boolean>("win_pause_download", { operationId });
  },
  winCancelDownload(operationId: string): Promise<boolean> {
    if (!hasTauriRuntime()) {
      return Promise.resolve(true);
    }
    return invoke<boolean>("win_cancel_download", { operationId });
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
    operationToken?: OperationToken,
  ): Promise<WinPerformReport> {
    if (!hasTauriRuntime()) {
      return confirm
        ? Promise.resolve(WIN_FALLBACK_PERFORM)
        : Promise.reject(new Error("explicit confirmation is required"));
    }
    if (!confirm) {
      return Promise.reject(new Error("explicit confirmation is required"));
    }
    const token = operationToken ?? (await managerApi.armDestructive("update"));
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
    return invoke<WinInstallStatus>("win_status").then(guardWinStatus);
  },
  winAdopt(): Promise<WinInstallStatus> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({ installed: WIN_FALLBACK_PLAN.installed, status: "managed" });
    }
    return invoke<WinInstallStatus>("win_adopt");
  },
  winPickExistingInstall(): Promise<WinInstallStatus["installed"] | null> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({
        path: "E:\\Tools\\Codex",
        version: "26.623.31921",
        arch: "x64",
        source: "portable",
        packageFamilyName: null,
        installedAt: Math.floor(Date.now() / 1000),
      });
    }
    return invoke<WinInstallStatus["installed"] | null>("win_pick_existing_install");
  },
  winAdoptPath(path: string): Promise<WinInstallStatus> {
    if (!hasTauriRuntime()) {
      return Promise.resolve({
        installed: {
          path,
          version: "26.623.31921",
          arch: "x64",
          source: "portable",
          packageFamilyName: null,
          installedAt: Math.floor(Date.now() / 1000),
        },
        status: "managed",
      });
    }
    return invoke<WinInstallStatus>("win_adopt_path", { path });
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
