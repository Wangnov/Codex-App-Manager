import type { TFn } from "../i18n";

export const MAX_LAUNCH_ENVIRONMENT_BYTES = 16 * 1024;

export const MANAGER_MANAGED_ENV_NAMES = [
  "CODEX_CLI_PATH",
  "CODEX_HOME",
  "CODEX_SPARKLE_ENABLED",
  "CODEX_WINDOWS_REGISTERED_CORE",
] as const;

export const BLOCKED_LAUNCH_ENV_NAMES = [
  // Windows core system, path, shell, and profile directories
  "PATH",
  "PATHEXT",
  "SYSTEMROOT",
  "SYSTEMDRIVE",
  "WINDIR",
  "COMSPEC",
  "USERPROFILE",
  "APPDATA",
  "LOCALAPPDATA",
  "TEMP",
  "TMP",
  "HOMEDRIVE",
  "HOMEPATH",
  "ALLUSERSPROFILE",
  "PROGRAMDATA",
  "PROGRAMFILES",
  "PROGRAMW6432",
  "COMMONPROGRAMFILES",
  "COMMONPROGRAMW6432",
  "PSMODULEPATH",
  "__COMPAT_LAYER",
  // Electron and Node.js code-injection, integrity-bypass, or TLS-disable variables
  "ELECTRON_RUN_AS_NODE",
  "ELECTRON_NO_ASAR",
  "ELECTRON_FORCE_IS_PACKAGED",
  "NODE_OPTIONS",
  "NODE_PATH",
  "NODE_TLS_REJECT_UNAUTHORIZED",
  // Dynamic library, OpenSSL, .NET profiler, and shell startup hijacking
  "LD_PRELOAD",
  "LD_LIBRARY_PATH",
  "LD_AUDIT",
  "DYLD_INSERT_LIBRARIES",
  "DYLD_LIBRARY_PATH",
  "DYLD_FRAMEWORK_PATH",
  "OPENSSL_CONF",
  "OPENSSL_ENGINES",
  "OPENSSL_MODULES",
  "COR_ENABLE_PROFILING",
  "COR_PROFILER",
  "COR_PROFILER_PATH",
  "COR_PROFILER_PATH_32",
  "COR_PROFILER_PATH_64",
  "CORECLR_ENABLE_PROFILING",
  "CORECLR_PROFILER",
  "CORECLR_PROFILER_PATH",
  "CORECLR_PROFILER_PATH_32",
  "CORECLR_PROFILER_PATH_64",
  "DOTNET_STARTUP_HOOKS",
  "BASH_ENV",
] as const;

const MANAGER_MANAGED_ENV_SET = new Set<string>(MANAGER_MANAGED_ENV_NAMES);
const BLOCKED_LAUNCH_ENV_SET = new Set<string>(BLOCKED_LAUNCH_ENV_NAMES);
const ENV_NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;
const CONTROL_OR_LINE_SEP_RE = /[\p{Cc}\u2028\u2029]/u;
const BLANK_RE = /^\p{White_Space}*$/u;
const UTF8_ENCODER = new TextEncoder();

export function launchEnvironmentError(text: string, t: TFn): string | null {
  if (UTF8_ENCODER.encode(text).length > MAX_LAUNCH_ENVIRONMENT_BYTES) {
    return t("settings.windows.envTooLong", { max: MAX_LAUNCH_ENVIRONMENT_BYTES });
  }
  const seen = new Set<string>();
  for (const [index, line] of text.replaceAll("\r\n", "\n").split("\n").entries()) {
    const row = index + 1;
    if (CONTROL_OR_LINE_SEP_RE.test(line)) {
      return t("settings.windows.envInvalid", { row });
    }
    if (BLANK_RE.test(line)) continue;
    const eq = line.indexOf("=");
    if (eq === -1) return t("settings.windows.envInvalid", { row });
    const name = line.slice(0, eq);
    const value = line.slice(eq + 1);
    if (!ENV_NAME_RE.test(name)) {
      return t("settings.windows.envInvalid", { row });
    }
    const canonical = name.toUpperCase();
    if (MANAGER_MANAGED_ENV_SET.has(canonical)) {
      return t("settings.windows.envReserved", { row });
    }
    if (BLOCKED_LAUNCH_ENV_SET.has(canonical)) {
      return t("settings.windows.envBlocked", { row });
    }
    if (seen.has(canonical)) {
      return t("settings.windows.envDuplicate", { row });
    }
    if (BLANK_RE.test(value)) {
      return t("settings.windows.envEmptyValue", { row });
    }
    seen.add(canonical);
  }
  return null;
}
