import { errorCode, errorMessage } from "../services/managerApi";
import type { TFn } from "./i18n";

export function userErrorMessage(cause: unknown, t: TFn): string {
  switch (errorCode(cause)) {
    case "install":
      return t("error.install");
    case "network":
    case "timeout":
      return t("home.error.network.sub");
    case "stale_expectation":
      return t("home.stale.rechecked");
    case "operation_busy":
      return t("error.busy");
    case "cancelled":
      return t("progress.cancelled");
    default:
      return errorMessage(cause);
  }
}
