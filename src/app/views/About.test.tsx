import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  IDLE_MANAGER_UPDATE_SNAPSHOT,
  managerApi,
  type ManagerUpdateAvailable,
} from "../../services/managerApi";
import { I18nProvider } from "../i18n";
import { ThemeProvider } from "../theme";
import { RELAUNCH_GRACE_MS } from "../ManagerUpdatePrompt";
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
      writeClipboardText: vi.fn((text: string) => navigator.clipboard.writeText(text)),
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
      code: null,
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
        code: null,
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
        code: null,
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
        code: "network",
        updatedAtMs: Date.now(),
      });
    });

    const dialog = await screen.findByRole("dialog");
    // The snapshot carries a stable code, which is localized; the sheet body
    // says the update failed (not that a check failed) and no raw engine text
    // is rendered.
    expect(within(dialog).getByRole("alert")).toHaveTextContent(
      "无法连接更新服务器。请检查网络后重试。",
    );
    expect(within(dialog).getByText("更新未能完成。")).toBeInTheDocument();
    expect(dialog).not.toHaveTextContent("install manager update");
    expect(dialog).not.toHaveTextContent("暂时无法检查管理器更新");
    await user.click(within(dialog).getByRole("button", { name: "重试" }));

    expect(api.ackManagerUpdateRuntime).toHaveBeenCalledTimes(1);
    await waitFor(() =>
      expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1),
    );
  });

  it("shows a blocked relaunch failure inside the open sheet, not just the background row", async () => {
    const user = userEvent.setup();
    let emit: ((snapshot: typeof IDLE_MANAGER_UPDATE_SNAPSHOT) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });
    // What the backend sends when another operation holds the lease.
    api.relaunchManager.mockRejectedValueOnce({
      code: "operation_busy",
      message: "已有操作正在进行（update），请等待完成后再试",
    });

    renderAbout();
    await waitFor(() => expect(emit).toBeDefined());

    act(() => {
      emit?.({
        phase: "installed",
        version: "0.5.4",
        downloaded: 100,
        total: 100,
        code: null,
        updatedAtMs: Date.now(),
      });
    });

    const dialog = await screen.findByRole("dialog");
    await user.click(
      within(dialog).getByRole("button", { name: "立即重启" }),
    );

    // The busy code maps to its own localized copy (not the generic
    // "something went wrong"), and it renders *inside the open sheet*, not in
    // the inert background row.
    expect(
      await within(dialog).findByRole("alert"),
    ).toHaveTextContent("已有操作正在进行，请稍后再试。");
  });

  it("keeps the relaunch recovery sheet even when a manual check finds an available update", async () => {
    const user = userEvent.setup();
    let emit: ((snapshot: typeof IDLE_MANAGER_UPDATE_SNAPSHOT) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });
    api.checkManagerUpdate.mockResolvedValue(available("0.5.4"));

    renderAbout();
    await waitFor(() => expect(emit).toBeDefined());

    act(() => {
      emit?.({
        phase: "installed",
        version: "0.5.4",
        downloaded: 100,
        total: 100,
        code: null,
        updatedAtMs: Date.now(),
      });
    });
    await user.click(screen.getByRole("button", { name: /检查管理器更新/ }));

    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByText("更新已安装，重新启动以应用。"),
    ).toBeInTheDocument();
    expect(
      within(dialog).getByRole("button", { name: "立即重启" }),
    ).toBeInTheDocument();
  });

  it("keeps a relaunch reminder reachable after cancelling an installed self-update", async () => {
    // Mirrors the same fix in ManagerUpdatePrompt: acking the runtime on a
    // plain Cancel would discard the only way left to relaunch bits that
    // are already installed on disk.
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
        code: null,
        updatedAtMs: 12345,
      });
    });

    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "取消" }));
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    expect(api.ackManagerUpdateRuntime).not.toHaveBeenCalled();

    const reminder = await screen.findByText("更新已安装，重新启动以应用。");
    await user.click(
      within(reminder.closest(".banner") as HTMLElement).getByRole("button", {
        name: "立即重启",
      }),
    );
    expect(api.relaunchManager).toHaveBeenCalledTimes(1);
  });

  it("acks the runtime only when the persistent reminder is explicitly dismissed", async () => {
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
        code: null,
        updatedAtMs: 999,
      });
    });

    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "取消" }));
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    expect(api.ackManagerUpdateRuntime).not.toHaveBeenCalled();

    const reminder = await screen.findByText("更新已安装，重新启动以应用。");
    await user.click(
      within(reminder.closest(".banner") as HTMLElement).getByRole("button", {
        name: "关闭",
      }),
    );
    expect(api.ackManagerUpdateRuntime).toHaveBeenCalledTimes(1);
  });

  it("does not flash the recovery sheet between an accepted relaunch and the process exiting", async () => {
    let emit: ((snapshot: typeof IDLE_MANAGER_UPDATE_SNAPSHOT) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });
    const install = vi.fn(async () => {
      emit?.({
        phase: "installed",
        version: "0.5.3",
        downloaded: 100,
        total: 100,
        code: null,
        updatedAtMs: Date.now(),
      });
    });
    api.checkManagerUpdate.mockResolvedValue(available("0.5.3", install));

    renderAbout();
    await waitFor(() => expect(emit).toBeDefined());
    fireEvent.click(screen.getByRole("button", { name: /检查管理器更新/ }));
    const dialog = await screen.findByRole("dialog", { name: "更新到 0.5.3?" });

    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      fireEvent.click(within(dialog).getByRole("button", { name: "更新" }));
      await act(async () => {});

      expect(install).toHaveBeenCalledTimes(1);
      const busy = within(screen.getByRole("dialog")).getByRole("button", {
        name: "正在重新启动…",
      });
      expect(busy).toBeDisabled();
      expect(screen.queryByText("更新已安装，重新启动以应用。")).toBeNull();
      expect(screen.queryByRole("button", { name: "立即重启" })).toBeNull();

      await act(() => vi.advanceTimersByTimeAsync(RELAUNCH_GRACE_MS));
      const recovery = screen.getByRole("dialog");
      expect(
        within(recovery).getByText("更新已安装，重新启动以应用。"),
      ).toBeInTheDocument();
      expect(
        within(recovery).getByRole("button", { name: "立即重启" }),
      ).toBeEnabled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("drops its own cached availability once the runtime reports that version installed", async () => {
    let emit: ((snapshot: typeof IDLE_MANAGER_UPDATE_SNAPSHOT) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });
    const pending = available("0.5.3");
    api.checkManagerUpdate.mockResolvedValue(pending);

    renderAbout();
    await waitFor(() => expect(emit).toBeDefined());
    fireEvent.click(screen.getByRole("button", { name: /检查管理器更新/ }));
    await screen.findByRole("dialog", { name: "更新到 0.5.3?" });

    // Another view (Home) completed the install of this exact version.
    act(() => {
      emit?.({
        phase: "installed",
        version: "0.5.3",
        downloaded: 100,
        total: 100,
        code: null,
        updatedAtMs: Date.now(),
      });
    });
    await waitFor(() => expect(pending.discard).toHaveBeenCalledTimes(1));
    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByText("更新已安装，重新启动以应用。"),
    ).toBeInTheDocument();

    // Once the runtime is acked back to idle nothing offers 0.5.3 again.
    act(() => emit?.(IDLE_MANAGER_UPDATE_SNAPSHOT));
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
  });

  it("shows the confirm sheet for a newer version found while the installed reminder is snoozed", async () => {
    const user = userEvent.setup();
    let emit: ((snapshot: typeof IDLE_MANAGER_UPDATE_SNAPSHOT) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });
    const newer = available("0.5.9");
    api.checkManagerUpdate.mockResolvedValue(newer);

    renderAbout();
    await waitFor(() => expect(emit).toBeDefined());
    act(() => {
      emit?.({
        phase: "installed",
        version: "0.5.4",
        downloaded: 100,
        total: 100,
        code: null,
        updatedAtMs: 1,
      });
    });
    const installed = await screen.findByRole("dialog");
    await user.click(within(installed).getByRole("button", { name: "取消" }));
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );

    await user.click(screen.getByRole("button", { name: /检查管理器更新/ }));
    const confirm = await screen.findByRole("dialog", { name: "更新到 0.5.9?" });
    expect(
      within(confirm).queryByRole("button", { name: "立即重启" }),
    ).toBeNull();
    await user.click(within(confirm).getByRole("button", { name: "取消" }));
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    expect(newer.discard).toHaveBeenCalled();
  });
});
