export type OperatingSystem = "windows" | "macos" | "linux" | "unknown";
export type Architecture = "x64" | "arm64" | "unknown";
export type OperationKind = "install" | "update" | "uninstall" | "set-install-root" | "adopt";
export type OperationToken = string;

/**
 * Serialized error returned by failing Tauri commands. Mirrors the backend
 * `CommandError` struct (src-tauri/src/errors.rs), which serializes
 * `#[serde(rename_all = "camelCase")]` from `AppError`.
 */
export interface CommandError {
  /**
   * Stable machine code, e.g. "unsupported_platform" | "engine_error" |
   * "stale_expectation" | "internal_error". `stale_expectation` means reality
   * no longer matches the snapshot the user confirmed — re-check, re-confirm.
   */
  code: string;
  /** Human-facing message (the `Display` of the underlying `AppError`). */
  message: string;
}

export interface InstalledCodex {
  path: string;
  build: number;
  /** Human-facing version (CFBundleShortVersionString, e.g. 26.602.40724). */
  shortVersion: string;
  arch: string;
  /** Bundle file mtime as a Unix timestamp (seconds) — when this build landed
   *  on disk (install or in-place update). Reliable fallback for a date. */
  installedAt?: number | null;
}

export interface UpdateStrategy {
  kind: "delta" | "full";
  fromBuild?: number;
}

export interface UpdatePlan {
  upToDate: boolean;
  currentBuild: number;
  latestBuild: number;
  latestShortVersion: string;
  strategy: UpdateStrategy;
  downloadUrl: string;
  downloadSize: number;
  edSignature: string | null;
  fullSize: number;
  savingsPct: number;
}

export interface DownloadProgress {
  downloaded: number;
  total: number;
  /** Host the bytes come from, e.g. codexapp.agentsmirror.com. */
  source: string;
}

export interface MacUpdateReport {
  appcastUrl: string;
  installed: InstalledCodex | null;
  simulatedBuild: number | null;
  plan: UpdatePlan | null;
  /** Sparkle <pubDate> of the appcast item matching the INSTALLED build (the
   *  true release date of the running version), when the feed publishes it. */
  installedPubDate?: string | null;
}

export interface MacPerformReport {
  upToDate: boolean;
  fromBuild: number;
  toBuild: number;
  strategy: string;
  installedPath: string;
  verified: boolean;
  relaunched: boolean;
  /** Codex was running but the relaunch failed — the user must start it
   *  manually. Distinct from `!relaunched`, which also covers the clean case
   *  where Codex simply wasn't running (no action needed). */
  relaunchFailed: boolean;
  rolledBack: boolean;
  /** Non-fatal warning to surface on an otherwise successful update (e.g. a
   *  provenance save failure, or where the previous backup was kept). null when
   *  the update was fully clean. */
  warning: string | null;
  message: string;
}

export type InstallClass = "managed" | "external" | "none";

export interface MacInstallStatus {
  installed: InstalledCodex | null;
  status: InstallClass;
}

export type UpdateSourceKind = "auto" | "mirror" | "official" | "custom";
export type WindowsInstallMode = "msix" | "portable";

export interface AppSettings {
  source: UpdateSourceKind;
  customUrl: string;
  autoCheck: boolean;
  askBefore: boolean;
  /** Always true at the backend; surfaced read-only in the UI. */
  signedOnly: boolean;
  /** Ask for confirmation before closing (quitting) the window. */
  confirmClose: boolean;
  /** Windows payload install preference. MSIX still falls back safely if blocked. */
  windowsInstallMode: WindowsInstallMode;
  /** Remembered portable install root for Windows. */
  installRoot: string;
}

export const DEFAULT_SETTINGS: AppSettings = {
  source: "auto",
  customUrl: "",
  autoCheck: true,
  askBefore: true,
  signedOnly: true,
  confirmClose: true,
  windowsInstallMode: "msix",
  installRoot: "%LOCALAPPDATA%\\Programs\\Codex",
};

export interface MacUninstallReport {
  removed: boolean;
  keptCodexHome: boolean;
  message: string;
}

export interface InstalledWindowsCodex {
  path: string;
  version: string;
  arch: string | null;
  source: "msix" | "portable" | string;
  packageFamilyName: string | null;
  /** Install-dir / executable mtime as a Unix timestamp (seconds). */
  installedAt?: number | null;
}

export interface WindowsRelease {
  version: string;
  packageMoniker: string;
  architecture: string | null;
  contentLength: number | null;
  etag: string | null;
  storeProductId: string | null;
  packageIdentity: string | null;
}

export type CapabilityState = "available" | "unavailable" | "unknown";

export interface CapabilityCheck {
  state: CapabilityState;
  detail: string;
}

export type SideloadRecommendation = "msix-preferred" | "portable-fallback";

export interface WinCapabilityReport {
  addAppxPackage: CapabilityCheck;
  appxService: CapabilityCheck;
  sideloadPolicy: CapabilityCheck;
  appInstaller: CapabilityCheck;
  /** Can the WinRT PackageManager actually activate? Catches the "registered but
   * broken" machines where MSIX deploy dies with 0x80040154 (没有注册类). */
  msixDeployment: CapabilityCheck;
  meteredNetwork: CapabilityCheck;
  recommendation: SideloadRecommendation;
  notes: string[];
}

export type WinInstallRoute = "msix-sideload" | "portable-fallback";

export interface WindowsUpdatePlan {
  upToDate: boolean;
  currentVersion: string | null;
  latestVersion: string;
  packageMoniker: string;
  packageUrl: string;
  downloadSize: number | null;
  sha256: string;
  route: WinInstallRoute;
  portableFallbackReady: boolean;
  warnings: string[];
}

export interface WinUpdateReport {
  manifestUrl: string;
  checksumsUrl: string;
  packageUrl: string;
  release: WindowsRelease;
  installed: InstalledWindowsCodex | null;
  capabilities: WinCapabilityReport;
  plan: WindowsUpdatePlan;
}

export interface AuthenticodeReport {
  trusted: boolean;
  publisherIsOpenai: boolean;
  status: string;
  statusMessage: string;
  subject: string;
  issuer: string;
  thumbprint: string;
}

export interface MsixIdentity {
  name: string;
  publisher: string;
  version: string;
  processorArchitecture: string;
}

export interface WinStageReport {
  upToDate: boolean;
  route: WinInstallRoute;
  latestVersion: string;
  packageMoniker: string;
  downloadSize: number;
  stagedPath: string | null;
  sha256: string;
  hashVerified: boolean;
  authenticode: AuthenticodeReport | null;
  identity: MsixIdentity | null;
  identityVerified: boolean;
  installReady: boolean;
  portableFallbackReady: boolean;
  notes: string[];
}

export interface MsixSideloadReport {
  success: boolean;
  message: string;
  installed: InstalledWindowsCodex | null;
  fallbackRecommended: boolean;
  rawError: string | null;
}

export interface MsixHealthReport {
  healthy: boolean;
  /**
   * Whether the health probe actually ran. When false, `healthy` is a
   * conservative "keep the MSIX" default (the probe could not run), not an
   * observed clean bill of health. Use this to tell "verified healthy" apart
   * from "kept because unverifiable".
   */
  verified: boolean;
  packageRegistered: boolean;
  /** Raw Get-AppxPackage Status string (e.g. "Ok", "Modified"). */
  status: string;
  statusOk: boolean;
  aumidResolved: boolean;
  /** Declared framework dependencies missing on this machine. */
  missingDependencies: string[];
  /** Human-facing reason when unhealthy; empty when healthy. */
  reason: string;
}

/**
 * Outcome of `win_perform_update`. Enumerates the exact action strings the
 * backend sets in src-tauri/src/app/win_update.rs:
 *   - "none"                                   — already up to date.
 *   - "msix-sideload"                          — MSIX sideload succeeded.
 *   - "portable-fallback"                      — user chose portable mode.
 *   - "portable-fallback-after-msix-failure"   — sideload failed, fell back.
 *   - "portable-fallback-after-msix-unhealthy" — sideload registered but the
 *                                                package failed its health check.
 *   - "portable-fallback-missing-framework"    — staged MSIX declared framework
 *                                                dependencies absent locally.
 */
export type WinPerformAction =
  | "none"
  | "msix-sideload"
  | "portable-fallback"
  | "portable-fallback-after-msix-failure"
  | "portable-fallback-after-msix-unhealthy"
  | "portable-fallback-missing-framework";

export interface WinPerformReport {
  success: boolean;
  action: WinPerformAction;
  message: string;
  stage: WinStageReport;
  sideload: MsixSideloadReport | null;
  portable: PortableInstallReport | null;
  msixHealth: MsixHealthReport | null;
  installed: InstalledWindowsCodex | null;
  fallbackAvailable: boolean;
  fallbackAttempted: boolean;
  notes: string[];
}

export interface PortableInstallReport {
  success: boolean;
  installRoot: string;
  executablePath: string | null;
  version: string;
  backupPath: string | null;
  shortcutCreated: boolean;
  uninstallEntryCreated: boolean;
  relaunched: boolean;
  message: string;
  notes: string[];
}

export interface MsixRemoveReport {
  success: boolean;
  message: string;
  rawError: string | null;
  notes: string[];
}

export interface PortableUninstallReport {
  success: boolean;
  partial: boolean;
  installRoot: string;
  removedFiles: boolean;
  removedShortcut: boolean;
  removedUninstallEntry: boolean;
  purgedUserData: boolean;
  message: string;
  notes: string[];
}

/**
 * Outcome of `win_uninstall`. Enumerates the exact action strings the backend
 * sets in src-tauri/src/app/win_update.rs:
 *   - "none"                 — nothing installed to remove.
 *   - "external-not-managed" — detected install isn't manager-managed; refused.
 *   - "remove-msix"          — removed the sideloaded MSIX package.
 *   - "remove-portable"      — removed the portable install.
 */
export type WinUninstallAction =
  | "none"
  | "external-not-managed"
  | "remove-msix"
  | "remove-portable";

export interface WinUninstallReport {
  success: boolean;
  action: WinUninstallAction;
  message: string;
  installedBefore: InstalledWindowsCodex | null;
  msix: MsixRemoveReport | null;
  portable: PortableUninstallReport | null;
  purgedUserData: boolean;
  notes: string[];
}

export interface WinInstallStatus {
  installed: InstalledWindowsCodex | null;
  status: InstallClass;
}
