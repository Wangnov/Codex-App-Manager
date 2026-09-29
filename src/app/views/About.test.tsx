import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  IDLE_MANAGER_UPDATE_SNAPSHOT,
  managerApi,
  type ManagerUpdateAvailable,
} from "../../services/managerApi";
import { I18nProvider } from "../i18n";
import { ThemeProvider } from "../theme";
import { About } from "./About";

vi.mock("../../services/managerApi", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("../../services/managerApi")>();
  return {
    ...actual,
    managerApi: {
      checkManagerUpdate: vi.fn(),
      openUrl: vi.fn(),
      openLogsDir: vi.fn(),
      getDiagnostics: vi.fn(),
      getManagerUpdateRuntime: vi
        .fn()
        .mockResolvedValue(actual.IDLE_MANAGER_UPDATE_SNAPSHOT),
      ackManagerUpdateRuntime: vi
        .fn()
        .mockResolvedValue(actual.IDLE_MANAGER_UPDATE_SNAPSHOT),
      onManagerUpdateRuntime: vi.fn().mockResolvedValue(() => {}),
      relaunchManager: vi.fn().mockResolvedValue(undefined),
    },
  };
});

const api = vi.mocked(managerApi);

function available(
  version: string,
  installAndRelaunch = vi.fn().mockResolvedValue(undefined),
): ManagerUpdateAvailable {
  return {
    kind: "available",
    version,
    currentVersion: "0.5.2",
    installAndRelaunch,
    discard: vi.fn().mockResolvedValue(undefined),
  };
}

function renderAbout() {
  return render(
    <ThemeProvider>
      <I18nProvider>
        <About onBack={vi.fn()} />
      </I18nProvider>
    </ThemeProvider>,
  );
}

describe("About manager update", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "zh-CN");
    api.checkManagerUpdate.mockReset();
    api.getManagerUpdateRuntime.mockReset();
    api.getManagerUpdateRuntime.mockResolvedValue(IDLE_MANAGER_UPDATE_SNAPSHOT);
    api.onManagerUpdateRuntime.mockReset();
    api.onManagerUpdateRuntime.mockResolvedValue(() => {});
    api.relaunchManager.mockReset();
    api.relaunchManager.mockResolvedValue(undefined);
  });

  it("really rechecks stale metadata and requires a fresh confirmation", async () => {
    const user = userEvent.setup();
    const staleInstall = vi.fn().mockRejectedValue({
      code: "stale_expectation",
      message: "release changed",
    });
    const stale = available("0.5.3", staleInstall);
    const fresh = available("0.5.4");
    api.checkManagerUpdate
      .mockResolvedValueOnce(stale)
      .mockResolvedValueOnce(fresh);

    renderAbout();
    await user.click(
      screen.getByRole("button", { name: /检查管理器更新/ }),
    );

    const staleDialog = await screen.findByRole("dialog", {
      name: "更新到 0.5.3?",
    });
    await user.click(
      within(staleDialog).getByRole("button", { name: "更新" }),
    );

    const freshDialog = await screen.findByRole("dialog", {
      name: "更新到 0.5.4?",
    });
    expect(freshDialog).toBeInTheDocument();
    expect(staleInstall).toHaveBeenCalledTimes(1);
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(2);
    expect(stale.discard).toHaveBeenCalledTimes(1);
    await waitFor(() =>
      expect(
        within(freshDialog).getByRole("button", { name: "更新" }),
      ).toBeEnabled(),
    );
  });

  it("shows the same backend-owned install progress a self-update started elsewhere reports", async () => {
    const user = userEvent.setup();
    let emit: ((snapshot: typeof IDLE_MANAGER_UPDATE_SNAPSHOT) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });
    let resolveInstall!: () => void;
    const install = vi.fn(
      () => new Promise<void>((resolve) => { resolveInstall = resolve; }),
    );
    api.checkManagerUpdate.mockResolvedValue(available("0.5.3", install));

    renderAbout();
    await user.click(screen.getByRole("button", { name: /检查管理器更新/ }));
    const dialog = await screen.findByRole("dialog", { name: "更新到 0.5.3?" });
    await user.click(within(dialog).getByRole("button", { name: "更新" }));
    await waitFor(() => expect(emit).toBeDefined());

    emit?.({
      phase: "downloading",
      version: "0.5.3",
      downloaded: 25,
      total: 100,
      message: null,
      updatedAtMs: Date.now(),
    });

    expect(await screen.findByRole("progressbar")).toHaveAttribute(
      "aria-valuenow",
      "25",
    );

    // installAndRelaunch normally never resolves in production — the process
    // exits from `manager_relaunch` first. Settle the promise only so the
    // test itself does not leave a dangling `act` warning.
    await act(async () => resolveInstall());
  });

  it("shows progress for a self-update About never started itself, with no pendingUpdate", async () => {
    // About here has not clicked "check for update" at all — `pendingUpdate`
    // and `mgrBusy` are at their initial values. A self-update started from
    // Home must still be visible purely from the shared runtime snapshot.
    let emit: ((snapshot: typeof IDLE_MANAGER_UPDATE_SNAPSHOT) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });

    renderAbout();
    await waitFor(() => expect(emit).toBeDefined());

    act(() => {
      emit?.({
        phase: "downloading",
        version: "0.5.4",
        downloaded: 25,
        total: 100,
        message: null,
        updatedAtMs: Date.now(),
      });
    });

    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    expect(await screen.findByRole("progressbar")).toHaveAttribute(
      "aria-valuenow",
      "25",
    );
    expect(api.checkManagerUpdate).not.toHaveBeenCalled();
  });

  it("reattaches to a self-update that finished installing elsewhere and offers a relaunch", async () => {
    const user = userEvent.setup();
    let emit: ((snapshot: typeof IDLE_MANAGER_UPDATE_SNAPSHOT) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });

    renderAbout();
    await waitFor(() => expect(emit).toBeDefined());

    act(() => {
      emit?.({
        phase: "installed",
        version: "0.5.4",
        downloaded: 100,
        total: 100,
        message: null,
        updatedAtMs: Date.now(),
      });
    });

    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByText("更新已安装，重新启动以应用。"),
    ).toBeInTheDocument();
    await user.click(
      within(dialog).getByRole("button", { name: "立即重启" }),
    );
    expect(api.relaunchManager).toHaveBeenCalledTimes(1);
  });

  it("reattaches to a self-update that failed elsewhere and lets the user retry", async () => {
    const user = userEvent.setup();
    let emit: ((snapshot: typeof IDLE_MANAGER_UPDATE_SNAPSHOT) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });
    api.checkManagerUpdate.mockResolvedValue({ kind: "none" });

    renderAbout();
    await waitFor(() => expect(emit).toBeDefined());

    act(() => {
      emit?.({
        phase: "error",
        version: "0.5.4",
        downloaded: 20,
        total: 100,
        message: "install manager update: network unreachable",
        updatedAtMs: Date.now(),
      });
    });

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("alert")).toHaveTextContent(
      "install manager update: network unreachable",
    );
    await user.click(within(dialog).getByRole("button", { name: "重试" }));

    expect(api.ackManagerUpdateRuntime).toHaveBeenCalledTimes(1);
    await waitFor(() =>
      expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1),
    );
  });
});
