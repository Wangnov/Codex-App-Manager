import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../services/managerApi";
import {
  invokeRootBackend,
  notifyRootFrontendFailure,
  operationRiskForSnapshot,
  RootCrashBoundary,
} from "./RootCrashBoundary";

vi.mock("../services/managerApi", () => ({
  errorMessage: (cause: unknown) => {
    if (cause instanceof Error) return cause.message;
    if (cause && typeof cause === "object" && "message" in cause) {
      return String((cause as { message: unknown }).message);
    }
    return String(cause);
  },
  managerApi: {
    reportFrontendError: vi.fn(() => Promise.resolve()),
    getOperationSnapshot: vi.fn(() => Promise.resolve(null)),
    confirmQuit: vi.fn(() => Promise.resolve()),
  },
}));

function Boom(): never {
  throw new Error("provider exploded");
}

function StringBoom(): never {
  throw "plain string failure";
}

beforeEach(() => {
  delete (
    window as typeof window & {
      __CAM_FRONTEND_READY__?: unknown;
      __TAURI_INTERNALS__?: unknown;
    }
  ).__CAM_FRONTEND_READY__;
  delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  localStorage.setItem("cam.lang", "en");
  vi.mocked(managerApi.reportFrontendError).mockResolvedValue(undefined);
  vi.mocked(managerApi.getOperationSnapshot).mockResolvedValue(null);
  vi.mocked(managerApi.confirmQuit).mockResolvedValue(undefined);
});

describe("RootCrashBoundary", () => {
  it("renders without providers and reports initial render failures", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    render(
      <RootCrashBoundary>
        <Boom />
      </RootCrashBoundary>,
    );

    expect(screen.getByRole("alert")).toHaveTextContent("The app shell could not start");
    expect(screen.getByRole("button", { name: "Reload interface" })).toBeInTheDocument();
    await waitFor(() =>
      expect(managerApi.reportFrontendError).toHaveBeenCalledWith(
        expect.objectContaining({ kind: "root.render", message: "provider exploded" }),
      ),
    );
    consoleError.mockRestore();
  });

  it("quits through the backend policy and surfaces a refusal", async () => {
    const user = userEvent.setup();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.mocked(managerApi.confirmQuit).mockRejectedValueOnce({
      code: "operation_not_interruptible",
      message: "install is committing",
    });
    render(
      <RootCrashBoundary>
        <Boom />
      </RootCrashBoundary>,
    );

    await user.click(screen.getByRole("button", { name: "Quit safely" }));
    await waitFor(() => expect(screen.getByText("install is committing")).toBeInTheDocument());
    expect(screen.queryByText("[object Object]")).not.toBeInTheDocument();
    consoleError.mockRestore();
  });

  it("normalizes non-Error render failures before reporting them", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    render(
      <RootCrashBoundary>
        <StringBoom />
      </RootCrashBoundary>,
    );

    expect(screen.getByText(/plain string failure/)).toBeInTheDocument();
    await waitFor(() =>
      expect(managerApi.reportFrontendError).toHaveBeenCalledWith(
        expect.objectContaining({ kind: "root.render", message: "plain string failure" }),
      ),
    );
    consoleError.mockRestore();
  });

  it("authenticates the crashed document before switching native quit delivery", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const invoke = vi.fn().mockResolvedValue(undefined);
    const host = window as typeof window & {
      __CAM_FRONTEND_READY__?: { generation: number; token: string };
      __TAURI_INTERNALS__?: { invoke: typeof invoke };
    };
    host.__CAM_FRONTEND_READY__ = { generation: 7, token: "document-token" };
    host.__TAURI_INTERNALS__ = { invoke };

    render(
      <RootCrashBoundary>
        <Boom />
      </RootCrashBoundary>,
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("frontend_failed", {
        generation: 7,
        token: "document-token",
      }),
    );
    consoleError.mockRestore();
  });

  it("still renders recovery controls when local storage is unavailable", () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const getItem = vi.spyOn(localStorage, "getItem").mockImplementation(() => {
      throw new DOMException("storage unavailable", "SecurityError");
    });

    try {
      render(
        <RootCrashBoundary>
          <Boom />
        </RootCrashBoundary>,
      );
      expect(screen.getByRole("button", { name: "Reload interface" })).toBeInTheDocument();
    } finally {
      getItem.mockRestore();
      consoleError.mockRestore();
    }
  });
});

describe("operationRiskForSnapshot", () => {
  it("distinguishes active, paused and protected phases", () => {
    expect(operationRiskForSnapshot(null)).toBe("idle");
    expect(operationRiskForSnapshot({ kind: "update", phase: "downloading" })).toBe("active");
    expect(operationRiskForSnapshot({ kind: "update", paused: true })).toBe("paused");
    expect(operationRiskForSnapshot({ kind: "install", phase: "committing" })).toBe("critical");
    expect(operationRiskForSnapshot({ kind: "install", interruptible: false })).toBe("critical");
  });

  it("treats a Manager self-update the same as a Codex install/update/uninstall", () => {
    // Regression: `manager-update` is a real `OperationKind` (the Manager's
    // own self-update), but the crash screen used to fall back to "idle" for
    // it — implying quitting was always safe, including mid-install.
    expect(
      operationRiskForSnapshot({ kind: "manager-update", phase: "downloading" }),
    ).toBe("active");
    expect(
      operationRiskForSnapshot({ kind: "manager-update", phase: "committing" }),
    ).toBe("critical");
  });
});

describe("invokeRootBackend", () => {
  it("uses the dependency-free Tauri bridge and rejects when it is unavailable", async () => {
    const host = window as typeof window & {
      __TAURI_INTERNALS__?: { invoke: ReturnType<typeof vi.fn> };
    };
    const previous = host.__TAURI_INTERNALS__;
    const invoke = vi.fn().mockResolvedValue({ kind: "update" });
    host.__TAURI_INTERNALS__ = { invoke };

    try {
      await expect(invokeRootBackend("get_operation_snapshot")).resolves.toEqual({
        kind: "update",
      });
      expect(invoke).toHaveBeenCalledWith("get_operation_snapshot", undefined);
      delete host.__TAURI_INTERNALS__;
      await expect(invokeRootBackend("confirm_quit")).rejects.toThrow(
        "Desktop backend unavailable",
      );
    } finally {
      if (previous === undefined) delete host.__TAURI_INTERNALS__;
      else host.__TAURI_INTERNALS__ = previous;
    }
  });

  it("waits for a late document token and normalizes synchronous bridge errors", async () => {
    const host = window as typeof window & {
      __CAM_FRONTEND_READY__?: { generation: number; token: string };
      __TAURI_INTERNALS__?: { invoke: ReturnType<typeof vi.fn> };
    };
    const invoke = vi.fn((_command: string, _args?: Record<string, unknown>): Promise<unknown> => {
      throw new Error("bridge threw");
    });
    host.__TAURI_INTERNALS__ = { invoke };

    await expect(invokeRootBackend("frontend_failed", {})).rejects.toThrow("bridge threw");
    invoke.mockResolvedValue(undefined);
    notifyRootFrontendFailure();
    expect(invoke).toHaveBeenCalledTimes(1);

    host.__CAM_FRONTEND_READY__ = { generation: 9, token: "late-token" };
    window.dispatchEvent(new Event("cam:frontend-readiness"));
    await waitFor(() =>
      expect(invoke).toHaveBeenLastCalledWith("frontend_failed", {
        generation: 9,
        token: "late-token",
      }),
    );
  });
});
