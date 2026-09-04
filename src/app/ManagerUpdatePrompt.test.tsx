import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  managerApi,
  SETTINGS_CHANGED_EVENT,
  type ManagerUpdateAvailable,
  type ManagerUpdateCheck,
} from "../services/managerApi";
import { DEFAULT_SETTINGS } from "../shared/types";
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
});
