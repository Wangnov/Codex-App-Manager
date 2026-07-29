import type { TFn, TKey } from "../../i18n";

const ERROR_KEYS: Record<string, TKey> = {
  desktop_required: "config.error.desktopRequired",
  orange_invalid_credentials: "config.error.invalidCredentials",
  orange_forbidden: "config.error.forbidden",
  orange_rate_limited: "config.error.rateLimited",
  orange_timeout: "config.error.timeout",
  orange_network: "config.error.network",
  orange_invalid_response: "config.error.invalidResponse",
  orange_api_rejected: "config.error.apiRejected",
  orange_2fa_unsupported: "config.error.twoFactorUnsupported",
  orange_turnstile_unsupported: "config.error.turnstileUnsupported",
  orange_credential_store: "config.error.credentialStore",
  orange_signed_out: "config.error.signedOut",
  orange_key_unavailable: "config.error.keyUnavailable",
  orange_persistence: "config.error.persistence",
  unsupported_platform: "config.error.unsupportedPlatform",
  provider_path_unavailable: "config.error.providerPathUnavailable",
  provider_unsafe_destination: "config.error.providerUnsafeDestination",
  provider_busy: "config.error.providerBusy",
  provider_empty_key: "config.error.providerEmptyKey",
  provider_io: "config.error.providerIo",
  ccswitch_unavailable: "config.error.ccsUnavailable",
};

export function apiConfigErrorText(code: string | null, t: TFn): string | null {
  if (!code) return null;
  return t(ERROR_KEYS[code] ?? "config.error.unknown");
}

export function apiConfigWarningText(code: string | null, t: TFn): string | null {
  if (!code) return null;
  if (code === "orange_credential_store") return t("config.rememberWarning");
  return apiConfigErrorText(code, t);
}
