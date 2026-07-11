import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { listen } from "@tauri-apps/api/event";

import { managerApi, type ManagerUpdateAvailable } from "../services/managerApi";
import type { ManagerUpdateRuntimeSnapshot } from "../shared/types";
import { I18nProvider } from "./i18n";
import { ThemeProvider } from "./theme";
import { About } from "./views/About";
import {
  MANAGER_UPDATE_COMPLETION_KEY,
  MANAGER_UPDATE_HANDOFF_GRACE_MS,
  MANAGER_UPDATE_STATE_EVENT,
  MANAGER_UPDATE_SNOOZE_KEY,
  ManagerUpdateBanner,
  ManagerUpdateProvider,
  ManagerUpdateSheet,
  useManagerUpdate,
} from "./managerUpdate";

vi.mock("../services/managerApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../services/managerApi")>();
  return {
    ...actual,
    managerApi: {
      ...actual.managerApi,
      checkManagerUpdate: vi.fn(),
      acknowledgeManagerUpdateRuntime: vi.fn(),
      getManagerUpdateRuntime: vi.fn(),
      installManagerUpdate: vi.fn(),
      relaunchManager: vi.fn(),
    },
  };
});

const api = vi.mocked(managerApi);
const listenMock = vi.mocked(listen);
const AVAILABLE: ManagerUpdateAvailable = {
  kind: "available",
  version: "2.0.0",
  currentVersion: "1.0.0",
  body: "# 2.0.0\n\nReliable delivery notes.",
};

function runtimeSnapshot(
  overrides: Partial<ManagerUpdateRuntimeSnapshot> = {},
): ManagerUpdateRuntimeSnapshot {
  return {
    revision: 1,
    version: AVAILABLE.version,
    currentVersion: AVAILABLE.currentVersion,
    body: AVAILABLE.body,
    phase: "downloading",
    downloaded: 0,
    total: null,
    failure: null,
    ...overrides,
  };
}

function Probe() {
  const state = useManagerUpdate();
  return (
    <div hidden>
      <output data-testid="manager-status">{state.status}</output>
      <output data-testid="manager-failure">{state.failure?.code ?? ""}</output>
    </div>
  );
}

function renderManager({
  currentVersion = "1.0.0",
  startupDelayMs = 0,
  periodicIntervalMs = 60_000,
  includeAbout = false,
}: {
  currentVersion?: string;
  startupDelayMs?: number;
  periodicIntervalMs?: number;
  includeAbout?: boolean;
} = {}) {
  return render(
    <ThemeProvider>
      <I18nProvider>
        <ManagerUpdateProvider
          currentVersion={currentVersion}
          startupDelayMs={startupDelayMs}
          periodicIntervalMs={periodicIntervalMs}
        >
          <Probe />
          <ManagerUpdateBanner />
          {includeAbout ? <About onBack={vi.fn()} /> : null}
          <ManagerUpdateSheet onOpenSettings={vi.fn()} />
        </ManagerUpdateProvider>
      </I18nProvider>
    </ThemeProvider>,
  );
}

describe("manager self-update state machine", () => {
  let onRuntime: ((event: { payload: ManagerUpdateRuntimeSnapshot }) => void) | undefined;

  beforeEach(() => {
    localStorage.setItem("cam.lang", "zh-CN");
    api.checkManagerUpdate.mockReset();
    api.acknowledgeManagerUpdateRuntime.mockReset();
    api.getManagerUpdateRuntime.mockReset();
    api.installManagerUpdate.mockReset();
    api.relaunchManager.mockReset();
    api.checkManagerUpdate.mockResolvedValue({ kind: "none" });
    api.acknowledgeManagerUpdateRuntime.mockResolvedValue(true);
    api.getManagerUpdateRuntime.mockResolvedValue(null);
    api.installManagerUpdate.mockResolvedValue(undefined);
    api.relaunchManager.mockResolvedValue(undefined);
    onRuntime = undefined;
    listenMock.mockImplementation((event, handler) => {
      if (event === MANAGER_UPDATE_STATE_EVENT) {
        onRuntime = handler as typeof onRuntime;
      }
      return Promise.resolve(() => {});
    });
  });

  it("checks on startup, presents notes on Home, and never installs without confirmation", async () => {
    const user = userEvent.setup();
    api.checkManagerUpdate.mockResolvedValue(AVAILABLE);
    renderManager();

    expect(await screen.findByText("发现管理器新版本 2.0.0")).toBeInTheDocument();
    expect(api.installManagerUpdate).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "查看说明" }));
    expect(await screen.findByRole("dialog")).toHaveTextContent("Reliable delivery notes.");
    expect(screen.getByLabelText("1.0.0 → 2.0.0")).toBeInTheDocument();

    await user.click(screen.getAllByRole("button", { name: "稍后提醒" })[1]);
    expect(screen.queryByText("发现管理器新版本 2.0.0")).not.toBeInTheDocument();
    expect(JSON.parse(localStorage.getItem(MANAGER_UPDATE_SNOOZE_KEY) ?? "null")).toEqual(
      expect.objectContaining({ version: "2.0.0" }),
    );
  });

  it("rechecks at the bounded periodic cadence without overlapping startup work", async () => {
    vi.useFakeTimers();
    try {
      let resolveStartup: (() => void) | undefined;
      api.checkManagerUpdate
        .mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              resolveStartup = () => resolve({ kind: "none" });
            }),
        )
        .mockResolvedValue({ kind: "none" });
      renderManager({ periodicIntervalMs: 60_000 });

      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      });

      await act(async () => {
        vi.advanceTimersByTime(1);
        await Promise.resolve();
      });
      expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);

      await act(async () => {
        vi.advanceTimersByTime(60_000);
        await Promise.resolve();
      });
      expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);

      await act(async () => {
        resolveStartup?.();
        await Promise.resolve();
      });
      await act(async () => {
        vi.advanceTimersByTime(60_000);
        await Promise.resolve();
      });
      expect(api.checkManagerUpdate).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps startup network failures silent until the user opens a recovery surface", async () => {
    api.checkManagerUpdate.mockRejectedValue({ code: "network", message: "offline" });
    renderManager();

    await waitFor(() =>
      expect(screen.getByTestId("manager-failure")).toHaveTextContent("network"),
    );
    expect(screen.getByTestId("manager-status")).toHaveTextContent("idle");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("keeps retry checks busy and offers Close after no update is found", async () => {
    const user = userEvent.setup();
    let resolveRetry: ((value: { kind: "none" }) => void) | undefined;
    api.checkManagerUpdate
      .mockRejectedValueOnce({ code: "network", message: "offline" })
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveRetry = resolve;
          }),
      );
    renderManager({ startupDelayMs: 60_000, includeAbout: true });

    await user.click(screen.getByRole("button", { name: /^检查管理器更新/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("无法连接更新服务器");

    await user.click(screen.getByRole("button", { name: "重试" }));
    expect(screen.getByTestId("manager-status")).toHaveTextContent("checking");
    expect(screen.getByRole("dialog")).toHaveTextContent("检查中…");
    expect(screen.getByRole("button", { name: "检查中…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "返回" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "稍后提醒" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "更新" })).not.toBeInTheDocument();

    await act(async () => {
      resolveRetry?.({ kind: "none" });
      await Promise.resolve();
    });
    expect(screen.getByTestId("manager-status")).toHaveTextContent("up-to-date");
    expect(screen.getByRole("dialog")).toHaveTextContent("管理器已是最新版。");
    await user.click(screen.getAllByRole("button", { name: "关闭" }).at(-1)!);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("renders real byte progress and preserves a retry path for offline failures", async () => {
    const user = userEvent.setup();
    let rejectInstall: ((cause: unknown) => void) | undefined;
    api.checkManagerUpdate.mockResolvedValue(AVAILABLE);
    api.installManagerUpdate.mockImplementation(
      () => new Promise<void>((_resolve, reject) => (rejectInstall = reject)),
    );
    const view = renderManager();

    await user.click(await screen.findByRole("button", { name: "查看说明" }));
    await user.click(screen.getByRole("button", { name: "更新" }));
    await waitFor(() => expect(onRuntime).toBeDefined());
    act(() => {
      onRuntime?.({
        payload: {
          revision: 2,
          version: "2.0.0",
          currentVersion: "1.0.0",
          body: AVAILABLE.body,
          phase: "downloading",
          downloaded: 5 * 1024 * 1024,
          total: 10 * 1024 * 1024,
          failure: null,
        },
      });
    });
    expect(screen.getByText("5.0 / 10.0 MiB")).toBeInTheDocument();
    expect(view.container.querySelector(".manager-update-progress .bar-fill")).toHaveStyle({
      width: "50%",
    });

    act(() => rejectInstall?.({ code: "network", message: "offline" }));
    expect(await screen.findByTestId("manager-failure")).toHaveTextContent("network");
    expect(screen.getByRole("alert")).toHaveTextContent("无法连接更新服务器");
    expect(screen.getByRole("button", { name: "网络" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "重试" })).toBeEnabled();
    expect(screen.getByText(/镜像与 GitHub/)).toBeInTheDocument();
  });

  it("blocks manager installation while a Codex operation owns the backend lock", async () => {
    const user = userEvent.setup();
    api.checkManagerUpdate.mockResolvedValue(AVAILABLE);
    api.installManagerUpdate.mockRejectedValue({
      code: "operation_busy",
      message: "Codex update is still committing",
    });
    renderManager();

    await user.click(await screen.findByRole("button", { name: "查看说明" }));
    await user.click(screen.getByRole("button", { name: "更新" }));

    expect(await screen.findByTestId("manager-failure")).toHaveTextContent("operation_busy");
    expect(screen.getByRole("alert")).toHaveTextContent("已有操作正在进行");
    expect(screen.queryByRole("button", { name: "网络" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重试" })).toBeEnabled();
  });

  it("keeps an installed marker and offers relaunch retry when automatic restart fails", async () => {
    const user = userEvent.setup();
    api.checkManagerUpdate.mockResolvedValue(AVAILABLE);
    api.relaunchManager
      .mockRejectedValueOnce(new Error("backend restart unavailable"))
      .mockRejectedValueOnce({
        code: "operation_busy",
        message: "Codex update started before relaunch retry",
      })
      .mockResolvedValueOnce(undefined);
    renderManager({ includeAbout: true });

    await user.click(await screen.findByRole("button", { name: "查看说明" }));
    await user.click(screen.getByRole("button", { name: "更新" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("更新已安装，但管理器未能自动重启");
    expect(JSON.parse(localStorage.getItem(MANAGER_UPDATE_COMPLETION_KEY) ?? "null")).toEqual(
      expect.objectContaining({ from: "1.0.0", to: "2.0.0", stage: "installed" }),
    );
    expect(
      screen.getAllByText("版本 2.0.0 已安装，需要重启管理器才能完成升级。"),
    ).toHaveLength(2);

    await user.click(screen.getByRole("button", { name: "取消" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.queryByRole("button", { name: "更新" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "立即重启" })).toBeEnabled();

    // A retry launched from the compact Home banner must recover when the
    // backend's atomic preflight finds a newly-started Codex operation.
    await user.click(screen.getByRole("button", { name: "立即重启" }));
    expect(api.relaunchManager).toHaveBeenCalledTimes(2);
    expect(await screen.findByRole("dialog")).toHaveTextContent(
      "更新已安装，但管理器未能自动重启",
    );
    await user.click(screen.getByRole("button", { name: "取消" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    // About shares the same state: its check entry only reopens the restart
    // surface and cannot check the feed or install the same package again.
    await user.click(screen.getByRole("button", { name: /^检查管理器更新/ }));
    expect(await screen.findByRole("dialog")).toHaveTextContent("需要重启管理器");
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);
    expect(api.installManagerUpdate).toHaveBeenCalledTimes(1);

    await user.click(screen.getAllByRole("button", { name: "立即重启" }).at(-1)!);
    expect(api.relaunchManager).toHaveBeenCalledTimes(3);
    expect(screen.getByTestId("manager-status")).toHaveTextContent("relaunching");
  });

  it("writes a download marker before invoking the platform updater", async () => {
    const user = userEvent.setup();
    api.checkManagerUpdate.mockResolvedValue(AVAILABLE);
    api.installManagerUpdate.mockImplementation(() => new Promise<void>(() => {}));
    const view = renderManager();

    await user.click(await screen.findByRole("button", { name: "查看说明" }));
    await user.click(screen.getByRole("button", { name: "更新" }));

    expect(JSON.parse(localStorage.getItem(MANAGER_UPDATE_COMPLETION_KEY) ?? "null")).toEqual(
      expect.objectContaining({ from: "1.0.0", to: "2.0.0", stage: "downloading" }),
    );
    view.unmount();
    localStorage.removeItem(MANAGER_UPDATE_COMPLETION_KEY);
  });

  it("reattaches to an in-flight manager update after a renderer reload", async () => {
    localStorage.setItem(
      MANAGER_UPDATE_COMPLETION_KEY,
      JSON.stringify({
        from: "1.0.0",
        to: "2.0.0",
        installedAt: Date.now(),
        stage: "downloading",
      }),
    );
    api.getManagerUpdateRuntime.mockResolvedValue(
      runtimeSnapshot({ downloaded: 5 * 1024 * 1024, total: 10 * 1024 * 1024 }),
    );
    const view = renderManager();

    expect(await screen.findByTestId("manager-status")).toHaveTextContent("downloading");
    expect(screen.getByText("5.0 / 10.0 MiB")).toBeInTheDocument();
    expect(view.container.querySelector(".manager-update-progress .bar-fill")).toHaveStyle({
      width: "50%",
    });
    expect(JSON.parse(localStorage.getItem(MANAGER_UPDATE_COMPLETION_KEY) ?? "null")).toEqual(
      expect.objectContaining({ from: "1.0.0", to: "2.0.0", stage: "downloading" }),
    );
    expect(api.checkManagerUpdate).not.toHaveBeenCalled();
  });

  it("discards an interrupted pre-handoff download without fencing a new process", async () => {
    localStorage.setItem(
      MANAGER_UPDATE_COMPLETION_KEY,
      JSON.stringify({
        from: "1.0.0",
        to: "2.0.0",
        installedAt: Date.now(),
        stage: "downloading",
      }),
    );
    renderManager({ currentVersion: "1.0.0" });

    expect(localStorage.getItem(MANAGER_UPDATE_COMPLETION_KEY)).toBeNull();
    expect(screen.getByTestId("manager-status")).not.toHaveTextContent("installing");
    await waitFor(() => expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1));
    expect(api.installManagerUpdate).not.toHaveBeenCalled();
  });

  it("restores a durable Windows handoff when the renderer never promoted its marker", async () => {
    localStorage.setItem(
      MANAGER_UPDATE_COMPLETION_KEY,
      JSON.stringify({
        from: "1.0.0",
        to: "2.0.0",
        installedAt: Date.now(),
        stage: "downloading",
      }),
    );
    api.getManagerUpdateRuntime.mockResolvedValue(
      runtimeSnapshot({
        revision: 1,
        phase: "installing",
        handoffStartedAt: Date.now(),
      }),
    );
    renderManager({ currentVersion: "1.0.0" });

    await waitFor(() =>
      expect(screen.getByTestId("manager-status")).toHaveTextContent("installing"),
    );
    expect(JSON.parse(localStorage.getItem(MANAGER_UPDATE_COMPLETION_KEY) ?? "null")).toEqual(
      expect.objectContaining({ from: "1.0.0", to: "2.0.0", stage: "installing" }),
    );
    expect(api.checkManagerUpdate).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "正在安装…" })).toBeDisabled();
  });

  it("does not let an older hydration snapshot overwrite a newer runtime event", async () => {
    let resolveRuntime: ((snapshot: ManagerUpdateRuntimeSnapshot | null) => void) | undefined;
    api.getManagerUpdateRuntime.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveRuntime = resolve;
        }),
    );
    renderManager();
    await waitFor(() => expect(onRuntime).toBeDefined());

    act(() => {
      onRuntime?.({ payload: runtimeSnapshot({ revision: 2, phase: "installing" }) });
    });
    expect(screen.getByTestId("manager-status")).toHaveTextContent("installing");
    expect(JSON.parse(localStorage.getItem(MANAGER_UPDATE_COMPLETION_KEY) ?? "null")).toEqual(
      expect.objectContaining({ from: "1.0.0", to: "2.0.0", stage: "installing" }),
    );

    await act(async () => {
      resolveRuntime?.(runtimeSnapshot({ revision: 1, phase: "downloading" }));
      await Promise.resolve();
    });
    expect(screen.getByTestId("manager-status")).toHaveTextContent("installing");
    expect(api.checkManagerUpdate).not.toHaveBeenCalled();
  });

  it("does not let an in-flight startup check overwrite a recovered runtime", async () => {
    let resolveCheck: ((value: typeof AVAILABLE) => void) | undefined;
    api.checkManagerUpdate.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveCheck = resolve;
        }),
    );
    renderManager();
    await waitFor(() => expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(onRuntime).toBeDefined());

    act(() => {
      onRuntime?.({ payload: runtimeSnapshot({ revision: 3, phase: "installing" }) });
    });
    await act(async () => {
      resolveCheck?.(AVAILABLE);
      await Promise.resolve();
    });

    expect(screen.getByTestId("manager-status")).toHaveTextContent("installing");
    expect(screen.getByRole("dialog")).toHaveTextContent("正在安装");
  });

  it("polls an active runtime to terminal state when event subscription fails", async () => {
    vi.useFakeTimers();
    try {
      listenMock.mockRejectedValueOnce(new Error("event bus unavailable"));
      api.getManagerUpdateRuntime
        .mockResolvedValueOnce(runtimeSnapshot({ revision: 1, phase: "downloading" }))
        .mockResolvedValueOnce(runtimeSnapshot({ revision: 2, phase: "installed" }));
      api.relaunchManager.mockRejectedValueOnce(new Error("relaunch unavailable"));
      renderManager();

      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(screen.getByTestId("manager-status")).toHaveTextContent("downloading");

      await act(async () => {
        vi.advanceTimersByTime(800);
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(api.getManagerUpdateRuntime).toHaveBeenCalledTimes(2);
      expect(api.relaunchManager).toHaveBeenCalledTimes(1);
      expect(screen.getByTestId("manager-status")).toHaveTextContent(
        "installed-awaiting-relaunch",
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("retries a failed initial runtime query while a provisional handoff exists", async () => {
    vi.useFakeTimers();
    try {
      localStorage.setItem(
        MANAGER_UPDATE_COMPLETION_KEY,
        JSON.stringify({
          from: "1.0.0",
          to: "2.0.0",
          installedAt: Date.now(),
          stage: "installing",
          handoffStartedAt: Date.now(),
        }),
      );
      listenMock.mockRejectedValueOnce(new Error("event bus unavailable"));
      api.getManagerUpdateRuntime
        .mockRejectedValueOnce(new Error("invoke bridge reloading"))
        .mockResolvedValueOnce(runtimeSnapshot({ revision: 1, phase: "downloading" }))
        .mockResolvedValueOnce(runtimeSnapshot({ revision: 2, phase: "installed" }));
      api.relaunchManager.mockRejectedValueOnce(new Error("relaunch unavailable"));
      renderManager();

      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(api.getManagerUpdateRuntime).toHaveBeenCalledTimes(1);
      expect(localStorage.getItem(MANAGER_UPDATE_COMPLETION_KEY)).not.toBeNull();
      expect(api.checkManagerUpdate).not.toHaveBeenCalled();

      await act(async () => {
        vi.advanceTimersByTime(800);
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(screen.getByTestId("manager-status")).toHaveTextContent("downloading");
      expect(api.checkManagerUpdate).not.toHaveBeenCalled();

      await act(async () => {
        vi.advanceTimersByTime(800);
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(api.relaunchManager).toHaveBeenCalledTimes(1);
      expect(screen.getByTestId("manager-status")).toHaveTextContent(
        "installed-awaiting-relaunch",
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("recovers a macOS terminal install and retries relaunch after renderer loss", async () => {
    localStorage.setItem(
      MANAGER_UPDATE_COMPLETION_KEY,
      JSON.stringify({
        from: "1.0.0",
        to: "2.0.0",
        installedAt: Date.now(),
        stage: "installing",
      }),
    );
    api.getManagerUpdateRuntime.mockResolvedValue(
      runtimeSnapshot({ revision: 4, phase: "installed", downloaded: 10, total: 10 }),
    );
    api.relaunchManager.mockRejectedValueOnce(new Error("backend restart unavailable"));
    renderManager();

    await waitFor(() => expect(api.relaunchManager).toHaveBeenCalledTimes(1));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "更新已安装，但管理器未能自动重启",
    );
    expect(screen.getByTestId("manager-status")).toHaveTextContent(
      "installed-awaiting-relaunch",
    );
    expect(JSON.parse(localStorage.getItem(MANAGER_UPDATE_COMPLETION_KEY) ?? "null")).toEqual(
      expect.objectContaining({ from: "1.0.0", to: "2.0.0", stage: "installed" }),
    );
  });

  it("does not fence a cold start for a non-durable macOS installing marker", async () => {
    localStorage.setItem(
      MANAGER_UPDATE_COMPLETION_KEY,
      JSON.stringify({
        from: "1.0.0",
        to: "2.0.0",
        installedAt: Date.now(),
        stage: "installing",
      }),
    );
    renderManager({ currentVersion: "1.0.0" });

    expect(localStorage.getItem(MANAGER_UPDATE_COMPLETION_KEY)).toBeNull();
    expect(screen.getByTestId("manager-status")).not.toHaveTextContent("installing");
    await waitFor(() => expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1));
    expect(api.installManagerUpdate).not.toHaveBeenCalled();
  });

  it("restores a terminal update failure instead of silently starting over", async () => {
    localStorage.setItem(
      MANAGER_UPDATE_COMPLETION_KEY,
      JSON.stringify({
        from: "1.0.0",
        to: "2.0.0",
        installedAt: Date.now(),
        stage: "installing",
      }),
    );
    api.getManagerUpdateRuntime.mockResolvedValue(
      runtimeSnapshot({
        revision: 3,
        phase: "error",
        failure: { code: "network", message: "mirror and GitHub failed" },
      }),
    );
    renderManager();

    expect(await screen.findByTestId("manager-failure")).toHaveTextContent("network");
    expect(screen.getByRole("alert")).toHaveTextContent("无法连接更新服务器");
    expect(localStorage.getItem(MANAGER_UPDATE_COMPLETION_KEY)).toBeNull();
    expect(api.checkManagerUpdate).not.toHaveBeenCalled();
  });

  it("CAS-acknowledges a stale terminal before refreshing consent", async () => {
    const user = userEvent.setup();
    const staleRuntime = runtimeSnapshot({
      revision: 5,
      phase: "error",
      failure: { code: "stale_expectation", message: "changed" },
    });
    api.checkManagerUpdate.mockResolvedValue(AVAILABLE);
    api.getManagerUpdateRuntime
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce(staleRuntime);
    api.installManagerUpdate.mockRejectedValueOnce({
      code: "stale_expectation",
      message: "changed",
    });
    renderManager();

    await user.click(await screen.findByRole("button", { name: "查看说明" }));
    await user.click(screen.getByRole("button", { name: "更新" }));

    await waitFor(() =>
      expect(api.acknowledgeManagerUpdateRuntime).toHaveBeenCalledWith(staleRuntime),
    );
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(2);
    expect(screen.getByTestId("manager-status")).toHaveTextContent("available");
    act(() => {
      onRuntime?.({ payload: staleRuntime });
    });
    expect(screen.getByTestId("manager-status")).toHaveTextContent("available");
  });

  it("uses the launched target version as proof of a Windows updater handoff", async () => {
    const user = userEvent.setup();
    localStorage.setItem(
      MANAGER_UPDATE_COMPLETION_KEY,
      JSON.stringify({
        from: "1.0.0",
        to: "2.0.0",
        installedAt: Date.now(),
        stage: "installing",
      }),
    );
    renderManager({ currentVersion: "2.0.0", startupDelayMs: 60_000 });

    expect(screen.getByText("管理器已升级到 2.0.0。")).toBeInTheDocument();
    await waitFor(() => expect(localStorage.getItem(MANAGER_UPDATE_COMPLETION_KEY)).toBeNull());
    await user.click(screen.getByRole("button", { name: "关闭" }));
    expect(screen.queryByText("管理器已升级到 2.0.0。")).not.toBeInTheDocument();
  });

  it("keeps a fresh Windows handoff fenced before expiring it", async () => {
    vi.useFakeTimers();
    try {
      localStorage.setItem(
        MANAGER_UPDATE_COMPLETION_KEY,
        JSON.stringify({
          from: "1.0.0",
          to: "2.0.0",
          installedAt: Date.now(),
          stage: "installing",
          handoffStartedAt: Date.now(),
        }),
      );
      renderManager({ currentVersion: "1.0.0", startupDelayMs: 1 });

      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(screen.getByTestId("manager-status")).toHaveTextContent("installing");
      expect(localStorage.getItem(MANAGER_UPDATE_COMPLETION_KEY)).not.toBeNull();
      expect(api.checkManagerUpdate).not.toHaveBeenCalled();
      expect(screen.getByRole("button", { name: "正在安装…" })).toBeDisabled();

      await act(async () => {
        vi.advanceTimersByTime(MANAGER_UPDATE_HANDOFF_GRACE_MS);
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(localStorage.getItem(MANAGER_UPDATE_COMPLETION_KEY)).toBeNull();
      expect(screen.getByTestId("manager-status")).toHaveTextContent("idle");

      await act(async () => {
        vi.advanceTimersByTime(1);
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);
      expect(api.installManagerUpdate).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("restores installed-awaiting-relaunch when the old process is opened again", async () => {
    localStorage.setItem(
      MANAGER_UPDATE_COMPLETION_KEY,
      JSON.stringify({
        from: "1.0.0",
        to: "2.0.0",
        installedAt: Date.now(),
        stage: "installed",
      }),
    );
    renderManager({ currentVersion: "1.0.0" });

    expect(screen.getByTestId("manager-status")).toHaveTextContent(
      "installed-awaiting-relaunch",
    );
    expect(
      screen.getByText("版本 2.0.0 已安装，需要重启管理器才能完成升级。"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "立即重启" })).toBeEnabled();
    await waitFor(() => expect(api.checkManagerUpdate).not.toHaveBeenCalled());
    expect(api.installManagerUpdate).not.toHaveBeenCalled();
  });
});
