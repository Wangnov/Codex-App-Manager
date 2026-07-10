import { errorCode, errorMessage } from "../services/managerApi";
import type { TFn, TKey } from "./i18n";

/**
 * Stable, localized failure surface for every user-visible error path.
 * Raw engine/OS detail is never the primary copy — it lives in `detail`
 * for a "Show details" disclosure only.
 */
export interface FailureSurface {
  /** Stable machine code from the backend, or "unknown". */
  code: string;
  /** Localized short message for banners / hero body. */
  message: string;
  /** Raw diagnostic for disclosure only; null when nothing useful remains. */
  detail: string | null;
  /** Whether the user can reasonably retry or change a setting to recover. */
  recoverable: boolean;
}

const CODE_COPY: Record<string, TKey> = {
  network: "error.network",
  timeout: "error.timeout",
  disk_space: "error.disk_space",
  disk_write: "error.disk_write",
  permission: "error.permission",
  signature: "error.signature",
  artifact: "error.artifact",
  incompatible: "error.incompatible",
  install: "error.install",
  cancelled: "progress.cancelled",
  operation_busy: "error.busy",
  stale_expectation: "home.stale.rechecked",
  unsupported_platform: "error.unsupported",
  internal_error: "error.generic",
  engine_error: "error.generic",
  contract_mismatch: "error.generic",
};

/** Codes where a retry / settings change / closing Codex is a reasonable next step. */
const RECOVERABLE = new Set([
  "network",
  "timeout",
  "disk_space",
  "disk_write",
  "permission",
  "artifact",
  "install",
  "cancelled",
  "operation_busy",
  "stale_expectation",
  "engine_error",
  "internal_error",
  "contract_mismatch",
  "unknown",
]);

export function resolveFailure(cause: unknown, t: TFn): FailureSurface {
  const code = errorCode(cause) ?? "unknown";
  const raw = errorMessage(cause).trim();
  const key = CODE_COPY[code] ?? "error.generic";
  const message = t(key);
  // Never surface raw engine / PowerShell / OS text as primary copy. Keep it
  // only when it actually adds diagnostic value beyond the localized line.
  const detail = raw && raw !== message && raw !== code ? raw : null;
  return {
    code,
    message,
    detail,
    recoverable: RECOVERABLE.has(code),
  };
}

/** Localized primary message only — safe for banners and toasts. */
export function userErrorMessage(cause: unknown, t: TFn): string {
  return resolveFailure(cause, t).message;
}

/** Build a FailureSurface from already-localized copy (no raw detail). */
export function messageFailure(
  message: string,
  code = "unknown",
  recoverable = true,
): FailureSurface {
  return { code, message, detail: null, recoverable };
}

/** True when the failure is a connectivity class (DNS / TLS / timeout). */
export function isConnectivityFailure(cause: unknown): boolean {
  const code = errorCode(cause);
  return code === "network" || code === "timeout";
}
