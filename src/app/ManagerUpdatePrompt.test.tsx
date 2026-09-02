import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  managerApi,
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

function PromptHost({ visible = true }: { visible?: boolean }) {
  const controller = useManagerUpdatePrompt();
  return visible ? <ManagerUpdatePrompt {...controller} /> : null;
}

function promptTree(visible = true) {
  return (
    <I18nProvider>
      <PromptHost visible={visible} />
    </I18nProvider>
  );
}

function renderPrompt() {
  return render(promptTree());
}

describe("ManagerUpdatePrompt", () => {
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
});
