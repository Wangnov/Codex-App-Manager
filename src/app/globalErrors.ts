import { managerApi } from "../services/managerApi";

let installed = false;

export function installGlobalErrorHandlers() {
  if (installed) {
    return;
  }
  installed = true;

  window.addEventListener("error", (event) => {
    const error = event.error instanceof Error ? event.error : new Error(String(event.message));
    void managerApi.reportFrontendError({
      kind: "window.error",
      message: error.message,
      stack: error.stack ?? null,
      componentStack: null,
    });
    window.dispatchEvent(new CustomEvent("cam:fatal", { detail: { error } }));
  });

  window.addEventListener("unhandledrejection", (event) => {
    const reason = event.reason;
    const error = reason instanceof Error ? reason : new Error(String(reason));
    void managerApi.reportFrontendError({
      kind: "unhandledrejection",
      message: error.message,
      stack: error.stack ?? null,
      componentStack: null,
    });
  });
}
