import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  IDLE_MANAGER_UPDATE_SNAPSHOT,
  managerApi,
  SETTINGS_CHANGED_EVENT,
  type ManagerUpdateAvailable,
  type ManagerUpdateCheck,
} from "../services/managerApi";
import { DEFAULT_SETTINGS, type ManagerUpdateSnapshot } from "../shared/types";
import { I18nProvider } from "./i18n";
import {
  ManagerUpdatePrompt,
  useManagerUpdatePrompt,
} from "./ManagerUpdatePrompt";

vi.mock("../services/managerApi", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("../services/managerApi")>();
  return {
    ...actual,
    managerApi: {
      getSettingsStrict: vi.fn(),
      checkManagerUpdate: vi.fn(),
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
  overrides: Partial<ManagerUpdateAvailable> = {},
): ManagerUpdateAvailable {
  return {
    kind: "available",
    version: "0.5.3",
    currentVersion: "0.5.2",
    installAndRelaunch: vi.fn().mockResolvedValue(undefined),
    discard: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
}

function PromptHost({ visible = true, manual = false }: { visible?: boolean; manual?: boolean }) {
  const controller = useManagerUpdatePrompt();
  return (
    <>
      {manual ? <button onClick={controller.check}>手动刷新</button> : null}
      {visible ? <ManagerUpdatePrompt {...controller} /> : null}
    </>
  );
}

function promptTree(visible = true, manual = false) {
  return (
    <I18nProvider>
      <PromptHost visible={visible} manual={manual} />
    </I18nProvider>
  );
}

function renderPrompt(manual = false) {
  return render(promptTree(true, manual));
}

describe("ManagerUpdatePrompt", () => {
  afterEach(() => vi.useRealTimers());

  beforeEach(() => {
    localStorage.setItem("cam.lang", "zh-CN");
    api.getSettingsStrict.mockReset();
    api.getSettingsStrict.mockResolvedValue(DEFAULT_SETTINGS);
    api.checkManagerUpdate.mockReset();
    api.getManagerUpdateRuntime.mockReset();
    api.getManagerUpdateRuntime.mockResolvedValue(IDLE_MANAGER_UPDATE_SNAPSHOT);
    api.onManagerUpdateRuntime.mockReset();
    api.onManagerUpdateRuntime.mockResolvedValue(() => {});
    api.relaunchManager.mockReset();
    api.relaunchManager.mockResolvedValue(undefined);
  });

  it("quietly checks on startup and stays hidden when no update is available", async () => {
    api.checkManagerUpdate.mockResolvedValue({ kind: "none" });

    const { container } = renderPrompt();

    await waitFor(() =>
      expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1),
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("does not contact the updater feed when startup checks are disabled", async () => {
    api.getSettingsStrict.mockResolvedValue({
      ...DEFAULT_SETTINGS,
      checkOnStartup: false,
    });

    const { container } = renderPrompt();
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(api.getSettingsStrict).toHaveBeenCalledTimes(1);
    expect(api.checkManagerUpdate).not.toHaveBeenCalled();
    expect(container).toBeEmptyDOMElement();
  });

  it("fails closed when the startup preference cannot be read", async () => {
    api.getSettingsStrict.mockRejectedValue(new Error("settings unavailable"));

    const { container } = renderPrompt();
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(api.getSettingsStrict).toHaveBeenCalledTimes(1);
    expect(api.checkManagerUpdate).not.toHaveBeenCalled();
    expect(container).toBeEmptyDOMElement();
  });

  it.each<ManagerUpdateCheck>([
    { kind: "development" },
    { kind: "unavailable" },
  ])("does not turn routine $kind checks into a warning", async (result) => {
    api.checkManagerUpdate.mockResolvedValue(result);

    const { container } = renderPrompt();

    await waitFor(() =>
      expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1),
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("keeps the startup checker mounted while operation screens hide the prompt", async () => {
    api.checkManagerUpdate.mockResolvedValue({ kind: "none" });
    const { rerender } = renderPrompt();
    await waitFor(() =>
      expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1),
    );

    rerender(promptTree(false));
    rerender(promptTree(true));
    await act(async () => {
      await Promise.resolve();
    });

    expect(api.getSettingsStrict).toHaveBeenCalledTimes(1);
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);
  });

  it("reuses the home banner and installs after an explicit confirmation", async () => {
    const user = userEvent.setup();
    const update = available();
    api.checkManagerUpdate.mockResolvedValue(update);

    renderPrompt();

    const message = await screen.findByText("发现管理器新版本 0.5.3");
    const banner = message.closest(".banner");
    expect(banner).not.toBeNull();
    await user.click(
      within(banner as HTMLElement).getByRole("button", { name: "更新" }),
    );

    const dialog = screen.getByRole("dialog", { name: "更新到 0.5.3?" });
    expect(
      within(dialog).getByText("将下载并安装管理器更新,完成后自动重启管理器。"),
    ).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "更新" }));

    await waitFor(() =>
      expect(update.installAndRelaunch).toHaveBeenCalledTimes(1),
    );
  });

  it("rechecks stale metadata and requires confirmation for the fresh version", async () => {
    const user = userEvent.setup();
    const stale = available({
      installAndRelaunch: vi.fn().mockRejectedValue({
        code: "stale_expectation",
        message: "release changed",
      }),
    });
    const fresh = available({ version: "0.5.4" });
    api.checkManagerUpdate
      .mockResolvedValueOnce(stale)
      .mockResolvedValueOnce(fresh);

    renderPrompt();

    const oldMessage = await screen.findByText("发现管理器新版本 0.5.3");
    await user.click(
      within(oldMessage.closest(".banner") as HTMLElement).getByRole("button", {
        name: "更新",
      }),
    );
    await user.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "更新" }),
    );

    expect(
      await screen.findByText("发现管理器新版本 0.5.4"),
    ).toBeInTheDocument();
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(2);
    expect(stale.discard).toHaveBeenCalledTimes(1);
    await waitFor(
      () => expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
      { timeout: 600 },
    );

    const freshMessage = screen.getByText("发现管理器新版本 0.5.4");
    await user.click(
      within(freshMessage.closest(".banner") as HTMLElement).getByRole(
        "button",
        { name: "更新" },
      ),
    );
    expect(
      screen.getByRole("dialog", { name: "更新到 0.5.4?" }),
    ).toBeInTheDocument();
  });

  it("clears stale metadata when the advertised update disappears", async () => {
    const user = userEvent.setup();
    const stale = available({
      installAndRelaunch: vi.fn().mockRejectedValue({
        code: "stale_expectation",
        message: "release disappeared",
      }),
    });
    api.checkManagerUpdate
      .mockResolvedValueOnce(stale)
      .mockResolvedValueOnce({ kind: "none" });

    renderPrompt();

    const message = await screen.findByText("发现管理器新版本 0.5.3");
    await user.click(
      within(message.closest(".banner") as HTMLElement).getByRole("button", {
        name: "更新",
      }),
    );
    await user.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "更新" }),
    );

    await waitFor(() =>
      expect(
        screen.queryByText("发现管理器新版本 0.5.3"),
      ).not.toBeInTheDocument(),
    );
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(2);
    expect(stale.discard).toHaveBeenCalledTimes(1);
  });

  it("keeps the confirmation actionable after an install failure", async () => {
    const user = userEvent.setup();
    const installAndRelaunch = vi
      .fn()
      .mockRejectedValue(new Error("network down"));
    api.checkManagerUpdate.mockResolvedValue(available({ installAndRelaunch }));

    renderPrompt();

    const message = await screen.findByText("发现管理器新版本 0.5.3");
    await user.click(
      within(message.closest(".banner") as HTMLElement).getByRole("button", {
        name: "更新",
      }),
    );
    const dialog = screen.getByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "更新" }));

    await screen.findByRole("alert");
    expect(within(dialog).getByRole("button", { name: "更新" })).toBeEnabled();
  });

  it("finds a release published after startup at the configured interval", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    api.getSettingsStrict.mockResolvedValue({
      ...DEFAULT_SETTINGS,
      periodicCheckIntervalSeconds: 120,
    });
    api.checkManagerUpdate.mockResolvedValueOnce({ kind: "none" }).mockResolvedValue(available());
    renderPrompt();
    await act(async () => {});
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);

    await act(() => vi.advanceTimersByTimeAsync(119_999));
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(await screen.findByText("发现管理器新版本 0.5.3")).toBeInTheDocument();
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(2);
  });

  it("runs periodic checks independently of startup checks and clamps the interval", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    api.getSettingsStrict.mockResolvedValue({
      ...DEFAULT_SETTINGS,
      checkOnStartup: false,
      periodicCheckIntervalSeconds: 1,
    });
    api.checkManagerUpdate.mockResolvedValue(available());
    renderPrompt();
    await act(async () => {});

    await act(() => vi.advanceTimersByTimeAsync(59_999));
    expect(api.checkManagerUpdate).not.toHaveBeenCalled();
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(await screen.findByText("发现管理器新版本 0.5.3")).toBeInTheDocument();
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);
  });

  it("allows an explicit manual check when both automatic settings are disabled", async () => {
    const user = userEvent.setup();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    api.getSettingsStrict.mockResolvedValue({
      ...DEFAULT_SETTINGS,
      checkOnStartup: false,
      periodicCheck: false,
    });
    api.checkManagerUpdate.mockResolvedValue(available());
    renderPrompt(true);
    await act(async () => {});
    await act(() => vi.advanceTimersByTimeAsync(3_600_000));
    expect(api.checkManagerUpdate).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "手动刷新" }));
    expect(await screen.findByText("发现管理器新版本 0.5.3")).toBeInTheDocument();
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);
  });

  it("applies saved interval and toggle changes without restarting", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const initial = { ...DEFAULT_SETTINGS, checkOnStartup: false, periodicCheck: false };
    api.getSettingsStrict.mockResolvedValue(initial);
    api.checkManagerUpdate.mockResolvedValue({ kind: "none" });
    renderPrompt();
    await act(async () => {});

    const save = (periodicCheck: boolean, periodicCheckIntervalSeconds: number) =>
      act(() => {
        window.dispatchEvent(new CustomEvent(SETTINGS_CHANGED_EVENT, {
          detail: { ...initial, periodicCheck, periodicCheckIntervalSeconds },
        }));
      });
    save(true, 120);
    await act(() => vi.advanceTimersByTimeAsync(120_000));
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);
    save(true, 180);
    await act(() => vi.advanceTimersByTimeAsync(120_000));
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);
    await act(() => vi.advanceTimersByTimeAsync(60_000));
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(2);
    save(false, 180);
    await act(() => vi.advanceTimersByTimeAsync(3_600_000));
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(2);
  });

  it("does not let a late settings read re-enable checks after they were disabled", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    let resolveSettings!: (value: typeof DEFAULT_SETTINGS) => void;
    api.getSettingsStrict.mockReturnValue(new Promise((resolve) => { resolveSettings = resolve; }));
    api.checkManagerUpdate.mockResolvedValue({ kind: "none" });
    renderPrompt();
    await act(async () => {
      window.dispatchEvent(new CustomEvent(SETTINGS_CHANGED_EVENT, {
        detail: { ...DEFAULT_SETTINGS, checkOnStartup: false, periodicCheck: false },
      }));
      resolveSettings(DEFAULT_SETTINGS);
    });
    await act(() => vi.advanceTimersByTimeAsync(3_600_000));
    expect(api.checkManagerUpdate).not.toHaveBeenCalled();
  });

  it("never schedules automatic checks after a settings failure but permits manual checks", async () => {
    const user = userEvent.setup();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    api.getSettingsStrict.mockRejectedValue(new Error("settings unavailable"));
    api.checkManagerUpdate.mockResolvedValue(available());
    renderPrompt(true);
    await act(async () => {});
    await act(() => vi.advanceTimersByTimeAsync(3_600_000));
    expect(api.checkManagerUpdate).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "手动刷新" }));
    expect(await screen.findByText("发现管理器新版本 0.5.3")).toBeInTheDocument();
  });

  it("coalesces startup, periodic and manual requests while the feed is pending", async () => {
    const user = userEvent.setup();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    let resolveCheck!: (value: ManagerUpdateCheck) => void;
    api.checkManagerUpdate.mockReturnValue(new Promise((resolve) => { resolveCheck = resolve; }));
    renderPrompt(true);
    await act(async () => {});
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);
    await act(() => vi.advanceTimersByTimeAsync(3_600_000));
    await user.click(screen.getByRole("button", { name: "手动刷新" }));
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);
    await act(async () => resolveCheck(available()));
    expect(await screen.findByText("发现管理器新版本 0.5.3")).toBeInTheDocument();
  });

  it("keeps a known update during transient feed failures but clears it after a successful empty result", async () => {
    const user = userEvent.setup();
    api.checkManagerUpdate.mockResolvedValueOnce(available())
      .mockResolvedValueOnce({ kind: "unavailable" })
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValueOnce({ kind: "none" });
    renderPrompt(true);
    await screen.findByText("发现管理器新版本 0.5.3");
    const manual = screen.getByRole("button", { name: "手动刷新" });
    await user.click(manual);
    expect(screen.getByText("发现管理器新版本 0.5.3")).toBeInTheDocument();
    await user.click(manual);
    expect(screen.getByText("发现管理器新版本 0.5.3")).toBeInTheDocument();
    await user.click(manual);
    expect(screen.queryByText("发现管理器新版本 0.5.3")).not.toBeInTheDocument();
  });

  it("does not replace the confirmed version with an earlier in-flight result and resumes after cancel", async () => {
    const user = userEvent.setup();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const old = available();
    const next = available({ version: "0.5.4" });
    let resolveCheck!: (value: ManagerUpdateCheck) => void;
    api.checkManagerUpdate.mockResolvedValueOnce(old)
      .mockImplementationOnce(() => new Promise((resolve) => { resolveCheck = resolve; }))
      .mockResolvedValue(next);
    renderPrompt(true);
    await screen.findByText("发现管理器新版本 0.5.3");
    await user.click(screen.getByRole("button", { name: "手动刷新" }));
    await user.click(screen.getByRole("button", { name: "更新" }));
    await act(async () => resolveCheck(next));
    expect(screen.getByRole("dialog", { name: "更新到 0.5.3?" })).toBeInTheDocument();
    expect(old.discard).not.toHaveBeenCalled();
    expect(next.discard).toHaveBeenCalledTimes(1);
    await act(() => vi.advanceTimersByTimeAsync(3_600_000));
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(2);
    await user.click(screen.getByRole("button", { name: "取消" }));
    await act(() => vi.advanceTimersByTimeAsync(900_000));
    expect(await screen.findByText("发现管理器新版本 0.5.4")).toBeInTheDocument();
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(3);
  });

  it("keeps checks paused until a failed installation is dismissed", async () => {
    const user = userEvent.setup();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    let rejectInstall!: (reason: Error) => void;
    const update = available({ installAndRelaunch: vi.fn(() => new Promise<void>((_, reject) => { rejectInstall = reject; })) });
    api.checkManagerUpdate.mockResolvedValue(update);
    renderPrompt();
    await user.click(await screen.findByRole("button", { name: "更新" }));
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "更新" }));
    await act(() => vi.advanceTimersByTimeAsync(900_000));
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);
    await act(async () => rejectInstall(new Error("offline")));
    await act(() => vi.advanceTimersByTimeAsync(900_000));
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: "取消" }));
    await act(() => vi.advanceTimersByTimeAsync(900_000));
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(2);
  });

  it("cleans up the timer and settings listener and discards late results on unmount", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    let resolveCheck!: (value: ManagerUpdateCheck) => void;
    api.checkManagerUpdate.mockReturnValue(new Promise((resolve) => { resolveCheck = resolve; }));
    const { unmount } = renderPrompt();
    await act(async () => {});
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);
    unmount();
    const update = available();
    await act(async () => {
      window.dispatchEvent(new CustomEvent(SETTINGS_CHANGED_EVENT, { detail: DEFAULT_SETTINGS }));
      resolveCheck(update);
    });
    await act(() => vi.advanceTimersByTimeAsync(3_600_000));
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);
    expect(update.discard).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("shows live download progress from the backend update runtime while installing", async () => {
    const user = userEvent.setup();
    let emit: ((snapshot: ManagerUpdateSnapshot) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });
    let resolveInstall!: () => void;
    const update = available({
      installAndRelaunch: vi.fn(
        () => new Promise<void>((resolve) => { resolveInstall = resolve; }),
      ),
    });
    api.checkManagerUpdate.mockResolvedValue(update);

    renderPrompt();
    await user.click(await screen.findByRole("button", { name: "更新" }));
    await user.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "更新" }),
    );
    await waitFor(() => expect(emit).toBeDefined());

    act(() => {
      emit?.({
        phase: "downloading",
        version: "0.5.3",
        downloaded: 50,
        total: 100,
        message: null,
        updatedAtMs: Date.now(),
      });
    });

    const bar = await screen.findByRole("progressbar");
    expect(bar).toHaveAttribute("aria-valuenow", "50");

    await act(async () => resolveInstall());
  });

  it("dismisses the banner as a remind-later and shows it again on the next check", async () => {
    const user = userEvent.setup();
    // Mirrors the real `checkManagerUpdate`, which hands back a fresh object
    // (new closures) on every call, even when nothing changed. `dismissed`
    // is compared by reference, so this is what lets the next check cycle
    // un-dismiss the banner.
    api.checkManagerUpdate.mockImplementation(() => Promise.resolve(available()));

    renderPrompt(true);
    await screen.findByText("发现管理器新版本 0.5.3");

    await user.click(screen.getByRole("button", { name: "关闭" }));
    expect(
      screen.queryByText("发现管理器新版本 0.5.3"),
    ).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "手动刷新" }));
    expect(
      await screen.findByText("发现管理器新版本 0.5.3"),
    ).toBeInTheDocument();
  });

  it("reattaches to a self-update that finished installing elsewhere and offers a relaunch", async () => {
    // No local `update` object was ever confirmed by this mount (e.g. a
    // renderer reload right as the install finished) — the recovery UI must
    // come entirely from the shared backend runtime snapshot.
    const user = userEvent.setup();
    api.checkManagerUpdate.mockResolvedValue({ kind: "none" });
    let emit: ((snapshot: ManagerUpdateSnapshot) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });

    renderPrompt();
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

  it("keeps the relaunch recovery sheet even when a startup check finds an available update", async () => {
    // The running process still reports its old version until it actually
    // relaunches, so a routine startup check can legitimately come back
    // "available" for the exact version the backend already reports
    // `installed` and awaiting relaunch. The recovery sheet must win instead
    // of being silently replaced by a fresh "update available" banner.
    const user = userEvent.setup();
    api.checkManagerUpdate.mockResolvedValue(available({ version: "0.5.4" }));
    let emit: ((snapshot: ManagerUpdateSnapshot) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });

    renderPrompt();
    await waitFor(() => expect(emit).toBeDefined());
    // The startup check runs and finds "available" — in a real reload this
    // races with (or follows) the runtime reporting `installed`.
    await waitFor(() =>
      expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1),
    );

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

    // No "found a new version" banner offering to install it again — just
    // the relaunch recovery sheet.
    expect(
      screen.queryByText("发现管理器新版本 0.5.4"),
    ).not.toBeInTheDocument();
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
    api.checkManagerUpdate.mockResolvedValue({ kind: "none" });
    let emit: ((snapshot: ManagerUpdateSnapshot) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });

    renderPrompt();
    await waitFor(() => expect(emit).toBeDefined());
    await waitFor(() => expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1));

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
      expect(api.checkManagerUpdate).toHaveBeenCalledTimes(2),
    );
  });

  it("closes the recovery sheet once the backend broadcasts the acked idle snapshot", async () => {
    // `retryAfterFailure` never touches `confirmOpen` — the sheet is only
    // open because `reattached` is true. It must close via the runtime
    // reverting to idle, which only happens if `manager_ack_update_runtime`
    // actually re-broadcasts on `manager://update-state` after a successful
    // ack (rather than just changing the backend's own copy silently).
    const user = userEvent.setup();
    api.checkManagerUpdate.mockResolvedValue({ kind: "none" });
    let emit: ((snapshot: ManagerUpdateSnapshot) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });
    api.ackManagerUpdateRuntime.mockImplementation(async () => {
      emit?.(IDLE_MANAGER_UPDATE_SNAPSHOT);
      return IDLE_MANAGER_UPDATE_SNAPSHOT;
    });

    renderPrompt();
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
    await user.click(within(dialog).getByRole("button", { name: "重试" }));

    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
  });

  it("unpauses checks so retrying a locally started install actually rechecks", async () => {
    // Regression: opening the confirm sheet from this mount's own banner
    // pauses periodic/automatic checks. If a self-update it started then
    // fails (the backend marks the runtime `error` and this mount reattaches
    // to its own failure, exactly as a separate reattach would), clicking
    // "retry" must unpause checks before refreshing — otherwise `check()`
    // silently no-ops forever and the banner/sheet can never come back.
    const user = userEvent.setup();
    let emit: ((snapshot: ManagerUpdateSnapshot) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });
    const update = available({
      installAndRelaunch: vi.fn(async () => {
        emit?.({
          phase: "error",
          version: "0.5.3",
          downloaded: 20,
          total: 100,
          message: "install manager update: network unreachable",
          updatedAtMs: Date.now(),
        });
        throw new Error("install manager update: network unreachable");
      }),
    });
    api.checkManagerUpdate
      .mockResolvedValueOnce(update)
      .mockResolvedValue({ kind: "none" });

    renderPrompt();
    await user.click(await screen.findByRole("button", { name: "更新" }));
    await user.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "更新" }),
    );

    const dialog = await screen.findByRole("dialog");
    const retryButton = await within(dialog).findByRole("button", {
      name: "重试",
    });
    await user.click(retryButton);

    expect(api.ackManagerUpdateRuntime).toHaveBeenCalledTimes(1);
    await waitFor(() =>
      expect(api.checkManagerUpdate).toHaveBeenCalledTimes(2),
    );
  });

  it("does not re-offer the same version after dismissing an installed self-update", async () => {
    // Regression: cancelling the "installed, awaiting relaunch" recovery
    // sheet acked the runtime back to idle but left the local `update`
    // object untouched, so the very next render re-satisfied `showBanner`
    // and offered to install the exact bits already on disk again, with no
    // way left to reach the relaunch action.
    const user = userEvent.setup();
    let emit: ((snapshot: ManagerUpdateSnapshot) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });
    api.ackManagerUpdateRuntime.mockImplementation(async () => {
      emit?.(IDLE_MANAGER_UPDATE_SNAPSHOT);
      return IDLE_MANAGER_UPDATE_SNAPSHOT;
    });
    const update = available({
      installAndRelaunch: vi.fn(async () => {
        emit?.({
          phase: "installed",
          version: "0.5.3",
          downloaded: 100,
          total: 100,
          message: null,
          updatedAtMs: Date.now(),
        });
      }),
    });
    api.checkManagerUpdate.mockResolvedValue(update);

    renderPrompt();
    await user.click(await screen.findByRole("button", { name: "更新" }));
    await user.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "更新" }),
    );

    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("更新已安装，重新启动以应用。");
    await user.click(within(dialog).getByRole("button", { name: "取消" }));

    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    expect(
      screen.queryByText("发现管理器新版本 0.5.3"),
    ).not.toBeInTheDocument();
  });

  it("keeps a relaunch reminder reachable after cancelling an installed self-update", async () => {
    // Regression: the previous fix (see above) suppressed the re-offer by
    // acking the runtime on Cancel, but that ack itself discarded the only
    // way left to relaunch the already-installed bits. Cancel must instead
    // leave a persistent reminder that can still relaunch — never a plain
    // ack that erases the recovery path entirely.
    const user = userEvent.setup();
    let emit: ((snapshot: ManagerUpdateSnapshot) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });
    const update = available({
      installAndRelaunch: vi.fn(async () => {
        emit?.({
          phase: "installed",
          version: "0.5.3",
          downloaded: 100,
          total: 100,
          message: null,
          updatedAtMs: 12345,
        });
      }),
    });
    api.checkManagerUpdate.mockResolvedValue(update);

    renderPrompt();
    await user.click(await screen.findByRole("button", { name: "更新" }));
    await user.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "更新" }),
    );

    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("更新已安装，重新启动以应用。");
    await user.click(within(dialog).getByRole("button", { name: "取消" }));
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );

    // The ack must NOT have been called by a plain Cancel — only the
    // reminder's own explicit close does that.
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
    api.checkManagerUpdate.mockResolvedValue({ kind: "none" });
    let emit: ((snapshot: ManagerUpdateSnapshot) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });

    renderPrompt();
    await waitFor(() => expect(emit).toBeDefined());

    act(() => {
      emit?.({
        phase: "installed",
        version: "0.5.4",
        downloaded: 100,
        total: 100,
        message: null,
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

  it("does not re-offer the same version after explicitly dismissing the persistent installed reminder", async () => {
    // Regression: dismissing the persistent "installed, awaiting relaunch"
    // reminder (its own close button, distinct from the sheet's Cancel)
    // acked the runtime back to idle but left the local `update` object
    // untouched. Once acked, `reattached` stops being true, so the stale
    // `update` alone re-satisfied `showBanner` and immediately re-offered
    // installing the exact bits already on disk, with no relaunch path left.
    const user = userEvent.setup();
    let emit: ((snapshot: ManagerUpdateSnapshot) => void) | undefined;
    api.onManagerUpdateRuntime.mockImplementation(async (onSnapshot) => {
      emit = onSnapshot;
      return () => {
        emit = undefined;
      };
    });
    api.ackManagerUpdateRuntime.mockImplementation(async () => {
      emit?.(IDLE_MANAGER_UPDATE_SNAPSHOT);
      return IDLE_MANAGER_UPDATE_SNAPSHOT;
    });
    const update = available({
      installAndRelaunch: vi.fn(async () => {
        emit?.({
          phase: "installed",
          version: "0.5.3",
          downloaded: 100,
          total: 100,
          message: null,
          updatedAtMs: Date.now(),
        });
      }),
    });
    api.checkManagerUpdate.mockResolvedValue(update);

    renderPrompt();
    await user.click(await screen.findByRole("button", { name: "更新" }));
    await user.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "更新" }),
    );

    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("更新已安装，重新启动以应用。");
    await user.click(within(dialog).getByRole("button", { name: "取消" }));
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );

    const reminder = await screen.findByText("更新已安装，重新启动以应用。");
    await user.click(
      within(reminder.closest(".banner") as HTMLElement).getByRole("button", {
        name: "关闭",
      }),
    );

    expect(api.ackManagerUpdateRuntime).toHaveBeenCalledTimes(1);
    await waitFor(() =>
      expect(
        screen.queryByText("更新已安装，重新启动以应用。"),
      ).not.toBeInTheDocument(),
    );
    expect(
      screen.queryByText("发现管理器新版本 0.5.3"),
    ).not.toBeInTheDocument();
  });
});
