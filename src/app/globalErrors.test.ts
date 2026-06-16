import { describe, expect, it, vi } from "vitest";

import { managerApi } from "../services/managerApi";
import { installGlobalErrorHandlers } from "./globalErrors";

vi.mock("../services/managerApi", () => ({
  managerApi: {
    reportFrontendError: vi.fn(() => Promise.resolve()),
  },
}));

const reportFrontendError = vi.mocked(managerApi.reportFrontendError);

describe("installGlobalErrorHandlers", () => {
  it("logs sync errors as fatal, logs promise rejections as non-fatal, and installs once", () => {
    const fatal = vi.fn();
    window.addEventListener("cam:fatal", fatal);

    installGlobalErrorHandlers();
    installGlobalErrorHandlers();

    const syncError = new Error("render broke");
    window.dispatchEvent(new ErrorEvent("error", { message: syncError.message, error: syncError }));

    expect(reportFrontendError).toHaveBeenCalledTimes(1);
    expect(reportFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "window.error", message: "render broke" }),
    );
    expect(fatal).toHaveBeenCalledTimes(1);

    const rejection = new Event("unhandledrejection") as PromiseRejectionEvent;
    Object.defineProperty(rejection, "reason", { value: new Error("background failed") });
    window.dispatchEvent(rejection);

    expect(reportFrontendError).toHaveBeenCalledTimes(2);
    expect(reportFrontendError).toHaveBeenLastCalledWith(
      expect.objectContaining({ kind: "unhandledrejection", message: "background failed" }),
    );
    expect(fatal).toHaveBeenCalledTimes(1);

    window.removeEventListener("cam:fatal", fatal);
  });
});
