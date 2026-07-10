import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../services/managerApi";
import { emptyOperationOutcome } from "../../shared/types";
import { I18nProvider } from "../i18n";
import { Uninstall } from "./Uninstall";

vi.mock("../../services/managerApi", () => ({
  managerApi: {
    macStatus: vi.fn(),
    winStatus: vi.fn(),
    macUninstall: vi.fn(),
    winUninstall: vi.fn(),
    retryAncillary: vi.fn(),
    armDestructive: vi.fn(() => Promise.resolve("token")),
    openCodexHome: vi.fn(() => Promise.resolve()),
  },
}));

const macStatus = vi.mocked(managerApi.macStatus);
const winStatus = vi.mocked(managerApi.winStatus);
const macUninstall = vi.mocked(managerApi.macUninstall);
const winUninstall = vi.mocked(managerApi.winUninstall);
const retryAncillary = vi.mocked(managerApi.retryAncillary);

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
    macUninstall.mockResolvedValue({
      removed: true,
      keptCodexHome: true,
      message: "removed",
      outcome: emptyOperationOutcome({
        primaryOk: true,
        appState: "absent",
        installClass: "none",
      }),
    });
    winUninstall.mockResolvedValue({
      success: true,
      action: "remove-portable",
      message: "removed",
      installedBefore: null,
      msix: null,
      portable: null,
      purgedUserData: false,
      notes: [],
      outcome: emptyOperationOutcome({
        primaryOk: true,
        appState: "absent",
        installClass: "none",
      }),
    });
    retryAncillary.mockResolvedValue({
      message: "cleanup ok",
      outcome: emptyOperationOutcome({
        primaryOk: true,
        appState: "absent",
        installClass: "none",
      }),
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

  it("does not mislabel a status-probe failure as external install", async () => {
    setPlatform("MacIntel");
    macStatus.mockRejectedValue(new Error("status probe failed"));
    renderUninstall();

    expect(
      await screen.findByText(/Could not confirm install status \(probe failed\)/i),
    ).toBeInTheDocument();
    expect(screen.queryByText(/external Codex/i)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Uninstall" })).toBeDisabled();
  });

  it("offers ancillary-only cleanup retry after partial uninstall", async () => {
    const user = userEvent.setup();
    setPlatform("Win32");
    winUninstall.mockResolvedValue({
      success: true,
      action: "remove-portable",
      message: "removed with cleanup warnings",
      installedBefore: null,
      msix: null,
      portable: {
        success: true,
        partial: true,
        installRoot: "C:\\Codex",
        removedFiles: true,
        removedShortcut: false,
        removedUninstallEntry: false,
        purgedUserData: false,
        message: "partial",
        notes: ["Start Menu shortcut cleanup failed: access denied"],
      },
      purgedUserData: false,
      notes: ["Start Menu shortcut cleanup failed: access denied"],
      outcome: emptyOperationOutcome({
        primaryOk: true,
        appState: "absent",
        installClass: "none",
        cleanup: { state: "failed", detail: "shortcut cleanup failed" },
        recoveryActions: ["cleanup_metadata"],
        warnings: ["Start Menu shortcut cleanup failed: access denied"],
      }),
    });
    renderUninstall();

    await screen.findByText("Data location");
    await user.click(screen.getByRole("button", { name: "Uninstall" }));
    await user.click(screen.getByRole("button", { name: "Continue" }));

    expect(
      await screen.findByText(/App removed, but some cleanup did not finish/i),
    ).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: /Retry cleanup only/i }),
    );
    await waitFor(() =>
      expect(retryAncillary).toHaveBeenCalledWith(
        expect.objectContaining({ actions: ["cleanup_metadata"] }),
      ),
    );
  });

  it("requires a second confirm before retrying purge_user_data", async () => {
    const user = userEvent.setup();
    setPlatform("MacIntel");
    macUninstall.mockResolvedValue({
      removed: true,
      keptCodexHome: true,
      message: "removed but purge failed",
      outcome: emptyOperationOutcome({
        primaryOk: true,
        appState: "absent",
        installClass: "none",
        path: "/Applications/Codex.app",
        cleanup: { state: "failed", detail: "purge failed" },
        recoveryActions: ["purge_user_data"],
        warnings: ["~/.codex purge failed"],
      }),
    });
    renderUninstall();

    await screen.findByText("Data location");
    await user.click(screen.getByRole("button", { name: "Uninstall" }));
    await user.click(screen.getByRole("button", { name: "Continue" }));

    expect(
      await screen.findByText(/App removed, but some cleanup did not finish/i),
    ).toBeInTheDocument();
    // Clicking the purge retry CTA opens a confirm sheet — no API call yet.
    await user.click(
      screen.getByRole("button", { name: /Retry purging user data only/i }),
    );
    expect(retryAncillary).not.toHaveBeenCalled();
    const purgeDialog = screen.getByRole("dialog", { name: "Erase all data?" });
    expect(purgeDialog).toBeInTheDocument();

    await user.click(
      within(purgeDialog).getByRole("button", {
        name: /Retry purging user data only/i,
      }),
    );
    await waitFor(() =>
      expect(retryAncillary).toHaveBeenCalledWith(
        expect.objectContaining({
          actions: ["purge_user_data"],
          purgeUserData: true,
          path: "/Applications/Codex.app",
        }),
      ),
    );
  });
});
