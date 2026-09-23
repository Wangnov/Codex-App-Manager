import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";

import type { Diagnostics, OperationSnapshot } from "../shared/types";
import { managerApi } from "../services/managerApi";
import { CATALOG, I18nProvider } from "./i18n";
import {
  crashBodyForSnapshot,
  ErrorBoundary,
  type CrashStrings,
} from "./ErrorBoundary";
import { QuitConfirm } from "./components";
import { ThemeProvider } from "./theme";

vi.mock("../services/managerApi", () => ({
  errorMessage: (cause: unknown) => {
    if (cause instanceof Error) return cause.message;
    if (cause && typeof cause === "object" && "message" in cause) {
      return String((cause as { message: unknown }).message);
    }
    return String(cause);
  },
  managerApi: {
    getDiagnostics: vi.fn(),
    reportFrontendError: vi.fn(() => Promise.resolve()),
    getOperationSnapshot: vi.fn(() => Promise.resolve(null)),
    confirmQuit: vi.fn(() => Promise.resolve()),
    frontendReady: vi.fn(() => Promise.resolve()),
    getSettings: vi.fn(() =>
      Promise.resolve({
        confirmClose: true,
      }),
    ),
  },
}));

const diagnostics: Diagnostics = {
  appVersion: "0.1.17",
  os: "macos",
  arch: "aarch64",
  locale: null,
  updateSource: "auto",
  customSourceHost: null,
  windowsInstallMode: null,
  installStatus: "macos status=managed",
  configHealth: {
    settingsStatus: "ok",
    provenanceStatus: "ok",
    unknownSource: null,
    detail: null,
    settingsBackupAvailable: false,
    provenanceBackupAvailable: false,
  },
  logsDir: "/tmp/logs",
  recentErrors: [],
  logTail: "",
  generatedAtUnix: 1,
};

function Boom(): never {
  throw new Error("boom");
}

const getDiagnostics = vi.mocked(managerApi.getDiagnostics);
const reportFrontendError = vi.mocked(managerApi.reportFrontendError);
const getOperationSnapshot = vi.mocked(managerApi.getOperationSnapshot);
const confirmQuit = vi.mocked(managerApi.confirmQuit);
const frontendReady = vi.mocked(managerApi.frontendReady);
const listenMock = vi.mocked(listen);

function enCrashStrings(): CrashStrings {
  const en = CATALOG.en;
  return {
    "crash.title": en["crash.title"],
    "crash.body": en["crash.body"],
    "crash.bodyActive": en["crash.bodyActive"],
    "crash.bodyCritical": en["crash.bodyCritical"],
    "crash.bodyPaused": en["crash.bodyPaused"],
    "crash.reload": en["crash.reload"],
    "crash.copy": en["crash.copy"],
    "crash.copied": en["crash.copied"],
    "crash.details": en["crash.details"],
    "crash.hideDetails": en["crash.hideDetails"],
    "crash.quit": en["crash.quit"],
  };
}

beforeEach(() => {
  localStorage.setItem("cam.lang", "en");
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    configurable: true,
    writable: true,
    value: {},
  });
  Object.defineProperty(window, "__CAM_FRONTEND_READY__", {
    configurable: true,
    writable: true,
    value: { generation: 1, token: "test-generation-token" },
  });
  getDiagnostics.mockResolvedValue(diagnostics);
  getOperationSnapshot.mockResolvedValue(null);
  confirmQuit.mockResolvedValue(undefined);
  frontendReady.mockResolvedValue(undefined);
  listenMock.mockResolvedValue(() => {});
});

describe("ErrorBoundary", () => {
  it("uses only the local close event in a browser preview", () => {
    delete (
      window as typeof window & { __TAURI_INTERNALS__?: unknown }
    ).__TAURI_INTERNALS__;

    render(
      <ThemeProvider>
        <I18nProvider>
          <QuitConfirm />
        </I18nProvider>
      </ThemeProvider>,
    );

    expect(listenMock).not.toHaveBeenCalled();
    expect(reportFrontendError).not.toHaveBeenCalled();
    expect(frontendReady).not.toHaveBeenCalled();
  });

  it("announces frontend readiness only after both native quit listeners register", async () => {
    render(
      <ThemeProvider>
        <I18nProvider>
          <QuitConfirm />
        </I18nProvider>
      </ThemeProvider>,
    );

    await waitFor(() => {
      expect(listenMock).toHaveBeenCalledWith("app://confirm-quit", expect.any(Function));
      expect(listenMock).toHaveBeenCalledWith("app://quit-blocked", expect.any(Function));
      expect(frontendReady).toHaveBeenCalledWith("en", 1, "test-generation-token");
    });
    expect(listenMock.mock.invocationCallOrder[1]).toBeLessThan(
      frontendReady.mock.invocationCallOrder[0],
    );
  });

  it("waits for the backend token before announcing the current document", async () => {
    delete (
      window as typeof window & { __CAM_FRONTEND_READY__?: unknown }
    ).__CAM_FRONTEND_READY__;

    render(
      <ThemeProvider>
        <I18nProvider>
          <QuitConfirm />
        </I18nProvider>
      </ThemeProvider>,
    );

    await waitFor(() => expect(listenMock).toHaveBeenCalledTimes(2));
    expect(frontendReady).not.toHaveBeenCalled();

    Object.defineProperty(window, "__CAM_FRONTEND_READY__", {
      configurable: true,
      writable: true,
      value: { generation: 2, token: "late-generation-token" },
    });
    window.dispatchEvent(new CustomEvent("cam:frontend-readiness"));

    await waitFor(() =>
      expect(frontendReady).toHaveBeenCalledWith("en", 2, "late-generation-token"),
    );
  });

  it("logs listener registration failure without falsely announcing readiness", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const consoleWarn = vi.spyOn(console, "warn").mockImplementation(() => {});
    listenMock.mockRejectedValue(new Error("listener unavailable"));

    render(
      <ThemeProvider>
        <I18nProvider>
          <QuitConfirm />
        </I18nProvider>
      </ThemeProvider>,
    );

    await waitFor(() =>
      expect(reportFrontendError).toHaveBeenCalledWith(
        expect.objectContaining({
          kind: "native-shell-listeners",
          message: "listener unavailable",
        }),
      ),
    );
    expect(frontendReady).not.toHaveBeenCalled();
    consoleWarn.mockRestore();
    consoleError.mockRestore();
  });

  it("retries partial listener registration and releases the orphaned listener once", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const orphanedUnlisten = vi.fn();
    listenMock
      .mockResolvedValueOnce(orphanedUnlisten)
      .mockRejectedValueOnce(new Error("second listener unavailable"));

    render(
      <ThemeProvider>
        <I18nProvider>
          <QuitConfirm />
        </I18nProvider>
      </ThemeProvider>,
    );

    await waitFor(
      () => expect(frontendReady).toHaveBeenCalledWith("en", 1, "test-generation-token"),
      { timeout: 2000 },
    );
    expect(orphanedUnlisten).toHaveBeenCalledTimes(1);
    expect(reportFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({
        kind: "native-shell-listeners",
        message: "second listener unavailable",
      }),
    );
    consoleError.mockRestore();
  });

  it("retries a transient frontend-ready IPC failure until queued events can drain", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    frontendReady.mockRejectedValueOnce(new Error("IPC warming up")).mockResolvedValue(undefined);

    render(
      <ThemeProvider>
        <I18nProvider>
          <QuitConfirm />
        </I18nProvider>
      </ThemeProvider>,
    );

    await waitFor(() => expect(frontendReady).toHaveBeenCalledTimes(2), { timeout: 2000 });
    expect(frontendReady).toHaveBeenNthCalledWith(1, "en", 1, "test-generation-token");
    expect(frontendReady).toHaveBeenNthCalledWith(2, "en", 1, "test-generation-token");
    expect(reportFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "native-shell-ready", message: "IPC warming up" }),
    );
    consoleError.mockRestore();
  });

  it("reruns an in-flight handshake with the replacement document generation and token", async () => {
    let resolveFirst: (() => void) | undefined;
    frontendReady
      .mockImplementationOnce(
        () =>
          new Promise<void>((resolve) => {
            resolveFirst = resolve;
          }),
      )
      .mockResolvedValue(undefined);

    render(
      <ThemeProvider>
        <I18nProvider>
          <QuitConfirm />
        </I18nProvider>
      </ThemeProvider>,
    );

    await waitFor(() =>
      expect(frontendReady).toHaveBeenCalledWith("en", 1, "test-generation-token"),
    );
    Object.defineProperty(window, "__CAM_FRONTEND_READY__", {
      configurable: true,
      writable: true,
      value: { generation: 2, token: "replacement-token" },
    });
    await act(async () => {
      window.dispatchEvent(new CustomEvent("cam:frontend-readiness"));
      resolveFirst?.();
    });

    await waitFor(() =>
      expect(frontendReady).toHaveBeenCalledWith("en", 2, "replacement-token"),
    );
  });

  it("renders a crash screen and copies diagnostics with the JS error", async () => {
    getDiagnostics.mockResolvedValue({
      ...diagnostics,
      os: "windows",
      windowsRuntime: ["bundled_executable_relocation_failed errorCode=EACCES"],
    });
    const user = userEvent.setup();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const writeText = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue(undefined);

    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>,
    );

    expect(screen.getByText(CATALOG.en["crash.title"])).toBeInTheDocument();
    expect(screen.queryByText("Error: boom")).not.toBeInTheDocument();
    expect(reportFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "render", message: "boom" }),
    );

    await user.click(screen.getByRole("button", { name: CATALOG.en["crash.details"] }));
    expect(screen.getByText("Error: boom")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: CATALOG.en["crash.copy"] }));
    await waitFor(() => expect(writeText).toHaveBeenCalled());
    expect(writeText.mock.calls[0][0]).toContain("## Frontend error");
    expect(writeText.mock.calls[0][0]).toContain("## Windows Codex runtime");
    expect(writeText.mock.calls[0][0]).toContain("bundled_executable_relocation_failed errorCode=EACCES");
    expect(
      screen.getByRole("button", { name: CATALOG.en["crash.copied"] }),
    ).toBeInTheDocument();

    writeText.mockRestore();
    consoleError.mockRestore();
  });

  it("shows the same crash screen for cam:fatal events", async () => {
    render(
      <ErrorBoundary>
        <div>healthy</div>
      </ErrorBoundary>,
    );

    window.dispatchEvent(new CustomEvent("cam:fatal", { detail: { error: new Error("fatal") } }));

    await waitFor(() =>
      expect(screen.getByText(CATALOG.en["crash.title"])).toBeInTheDocument(),
    );
    expect(screen.queryByText("Error: fatal")).not.toBeInTheDocument();
  });

  it("uses the current locale for crash copy (zh-CN, de, ar)", () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});

    for (const lang of ["zh-CN", "de", "ar"] as const) {
      localStorage.setItem("cam.lang", lang);
      const { unmount } = render(
        <ErrorBoundary>
          <Boom />
        </ErrorBoundary>,
      );
      expect(screen.getByText(CATALOG[lang]["crash.title"])).toBeInTheDocument();
      expect(screen.getByText(CATALOG[lang]["crash.body"])).toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: CATALOG[lang]["crash.reload"] }),
      ).toBeInTheDocument();
      unmount();
    }

    consoleError.mockRestore();
  });

  it("uses backend operation state for crash-page copy when an update is active", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const snap: OperationSnapshot = {
      id: "op-1",
      kind: "update",
      phase: "downloading",
      progress: { downloaded: 1, total: 2, source: "x" },
      paused: false,
      cancellable: true,
      interruptible: true,
    };
    getOperationSnapshot.mockResolvedValue(snap);

    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>,
    );

    await waitFor(() => {
      expect(screen.getByText(CATALOG.en["crash.bodyActive"])).toBeInTheDocument();
    });
    expect(getOperationSnapshot).toHaveBeenCalled();
    consoleError.mockRestore();
  });

  it("warns not to force-quit when the op is at a critical phase", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    getOperationSnapshot.mockResolvedValue({
      id: "op-2",
      kind: "install",
      phase: "committing",
      progress: null,
      paused: false,
      cancellable: false,
      interruptible: false,
    });

    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>,
    );

    await waitFor(() => {
      expect(screen.getByText(CATALOG.en["crash.bodyCritical"])).toBeInTheDocument();
    });
    consoleError.mockRestore();
  });

  it("uses the backend phase-aware quit command without depending on QuitConfirm", async () => {
    const user = userEvent.setup();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    delete (
      window as typeof window & { __TAURI_INTERNALS__?: unknown }
    ).__TAURI_INTERNALS__;

    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>,
    );

    expect(screen.getByRole("button", { name: CATALOG.en["crash.quit"] })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: CATALOG.en["crash.quit"] }));
    await waitFor(() => expect(confirmQuit).toHaveBeenCalled());

    consoleError.mockRestore();
  });

  it("surfaces a serialized backend refusal message", async () => {
    const user = userEvent.setup();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    confirmQuit.mockRejectedValueOnce({
      code: "operation_not_interruptible",
      message: "install is committing",
    });

    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>,
    );
    await user.click(screen.getByRole("button", { name: CATALOG.en["crash.quit"] }));

    await waitFor(() => expect(screen.getByText("install is committing")).toBeInTheDocument());
    expect(screen.queryByText("[object Object]")).not.toBeInTheDocument();
    consoleError.mockRestore();
  });
});

describe("crashBodyForSnapshot", () => {
  const en = enCrashStrings();

  it("selects idle / active / critical / paused copy", () => {
    expect(crashBodyForSnapshot(en, null)).toBe(en["crash.body"]);
    expect(
      crashBodyForSnapshot(en, {
        id: "1",
        kind: "update",
        phase: "downloading",
        progress: null,
        paused: false,
        cancellable: true,
        interruptible: true,
      }),
    ).toBe(en["crash.bodyActive"]);
    expect(
      crashBodyForSnapshot(en, {
        id: "1",
        kind: "install",
        phase: "committing",
        progress: null,
        paused: false,
        cancellable: false,
        interruptible: false,
      }),
    ).toBe(en["crash.bodyCritical"]);
    expect(
      crashBodyForSnapshot(en, {
        id: "1",
        kind: "update",
        phase: "downloading",
        progress: null,
        paused: true,
        cancellable: true,
        interruptible: true,
      }),
    ).toBe(en["crash.bodyPaused"]);
  });
});
