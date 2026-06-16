import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../services/managerApi";
import { I18nProvider } from "../i18n";
import { Uninstall } from "./Uninstall";

vi.mock("../../services/managerApi", () => ({
  errorMessage: (cause: unknown) => (cause instanceof Error ? cause.message : String(cause)),
  managerApi: {
    macStatus: vi.fn(),
    winStatus: vi.fn(),
    macUninstall: vi.fn(),
    winUninstall: vi.fn(),
    openCodexHome: vi.fn(() => Promise.resolve()),
  },
}));

const macStatus = vi.mocked(managerApi.macStatus);
const winStatus = vi.mocked(managerApi.winStatus);
const macUninstall = vi.mocked(managerApi.macUninstall);
const winUninstall = vi.mocked(managerApi.winUninstall);

function setPlatform(platform: string) {
  Object.defineProperty(navigator, "platform", { configurable: true, value: platform });
}

function renderUninstall() {
  return render(
    <I18nProvider>
      <Uninstall onBack={vi.fn()} />
    </I18nProvider>,
  );
}

describe("Uninstall", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "en");
    macStatus.mockResolvedValue({ status: "managed", installed: null });
    winStatus.mockResolvedValue({ status: "managed", installed: null });
    macUninstall.mockResolvedValue({ removed: true, keptCodexHome: true, message: "removed" });
    winUninstall.mockResolvedValue({
      success: true,
      action: "remove-portable",
      message: "removed",
      installedBefore: null,
      msix: null,
      portable: null,
      purgedUserData: false,
      notes: [],
    });
  });

  it("shows and copies the Windows Codex data path", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue(undefined);
    setPlatform("Win32");
    renderUninstall();

    expect(await screen.findByText("Data location")).toBeInTheDocument();
    expect(screen.getByText("%USERPROFILE%\\.codex")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Copy path" }));

    expect(writeText).toHaveBeenCalledWith("%USERPROFILE%\\.codex");
    expect(screen.getByRole("button", { name: "Path copied" })).toBeInTheDocument();
    writeText.mockRestore();
  });

  it("uninstalls after one confirmation when keeping data", async () => {
    const user = userEvent.setup();
    setPlatform("MacIntel");
    renderUninstall();

    await screen.findByText("Data location");
    await user.click(screen.getByRole("button", { name: "Uninstall" }));
    expect(screen.getByRole("dialog", { name: "Uninstall Codex?" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Continue" }));

    await waitFor(() => expect(macUninstall).toHaveBeenCalledWith(true));
  });

  it("requires a second confirmation before purging data", async () => {
    const user = userEvent.setup();
    setPlatform("MacIntel");
    renderUninstall();

    await screen.findByText("Data location");
    await user.click(screen.getByRole("switch"));
    await user.click(screen.getByRole("button", { name: "Uninstall" }));
    await user.click(screen.getByRole("button", { name: "Continue" }));

    expect(macUninstall).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "Erase all data?" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Erase & uninstall" }));

    await waitFor(() => expect(macUninstall).toHaveBeenCalledWith(false));
  });
});
