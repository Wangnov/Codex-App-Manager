import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { I18nProvider } from "../i18n";
import { InstallOtherVersionSheet } from "./InstallOtherVersion";

function renderPicker(
  platform: "macos" | "windows",
  currentVersion: string | null =
    platform === "macos" ? "26.806.12001" : "26.806.12001.0",
  architecture: string | null = "x64",
) {
  return render(
    <I18nProvider>
      <InstallOtherVersionSheet
        open
        platform={platform}
        currentVersion={currentVersion}
        architecture={architecture}
        onDismiss={vi.fn()}
      />
    </I18nProvider>,
  );
}

describe("InstallOtherVersionSheet", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "en");
  });

  it("selects a historical GitHub Release and keeps self-updates blocked by default", async () => {
    const user = userEvent.setup();
    renderPicker("macos");

    expect(screen.getByRole("dialog", { name: "Choose install version" })).toBeInTheDocument();
    expect(screen.getByText("GitHub Releases")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /26\.727\.51351.*GitHub Releases/ })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /26\.727\.51351/ }));

    expect(screen.getByText("GitHub Releases · x64 · DMG / ZIP")).toBeInTheDocument();
    expect(screen.getByRole("switch")).toBeChecked();
    expect(screen.getByRole("button", { name: "Download and install" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Download and install" }));
    expect(screen.getByRole("heading", { name: "Preview complete" })).toBeInTheDocument();
  });

  it("accepts GitHub Release DMG and ZIP assets on macOS without a Sparkle path", async () => {
    const user = userEvent.setup();
    renderPicker("macos", null);

    expect(screen.getByText(".dmg")).toBeInTheDocument();
    expect(screen.getByText(".zip")).toBeInTheDocument();
    expect(screen.queryByText(/sparkle/i)).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Install from a local package/ }));

    expect(screen.getByText("Codex-mac-x64.dmg")).toBeInTheDocument();
    expect(
      screen.getByText("Verify the local package and install 26.727.51351."),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Verify and install" })).toBeInTheDocument();
  });

  it("matches a Windows package-version fallback and keeps the local entry responsive", async () => {
    const user = userEvent.setup();
    renderPicker("windows", "26.727.6591.0");

    expect(screen.queryByRole("button", { name: /26\.727\.51351/ })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Install from a local package/ }));

    expect(
      screen.getByText("OpenAI.Codex_26.721.11231.0_x64__2p2nqsd0c76g0.Msix"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Verify and install" })).toBeInTheDocument();
  });

  it("only offers MSIX as the local GitHub Release asset on Windows", () => {
    renderPicker("windows");

    expect(screen.getByText(".msix")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /26\.727\.51351/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /26\.727\.6591\.0/ })).not.toBeInTheDocument();
    expect(screen.queryByText(".dmg")).not.toBeInTheDocument();
    expect(screen.queryByText(".zip")).not.toBeInTheDocument();
  });

  it("uses the detected ARM64 architecture for labels and local assets", async () => {
    const user = userEvent.setup();
    renderPicker("windows", null, "aarch64");

    expect(screen.getByText("Windows · arm64")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Install from a local package/ }));

    expect(
      screen.getByText("OpenAI.Codex_26.727.6591.0_arm64__2p2nqsd0c76g0.Msix"),
    ).toBeInTheDocument();
    expect(screen.getByText("MSIX · arm64")).toBeInTheDocument();
  });

  it("requires an explicit architecture when no reliable native architecture is available", async () => {
    const user = userEvent.setup();
    renderPicker("windows", null, null);

    const localPackage = screen.getByRole("button", { name: /Install from a local package/ });
    const historicalVersion = screen.getByRole("button", { name: /26\.727\.51351/ });
    expect(localPackage).toBeDisabled();
    expect(historicalVersion).toBeDisabled();
    expect(screen.getByRole("button", { name: "arm64" })).toHaveFocus();

    await user.click(screen.getByRole("button", { name: "arm64" }));
    expect(localPackage).toBeEnabled();
    expect(historicalVersion).toBeEnabled();
    expect(historicalVersion).toHaveFocus();

    await user.click(localPackage);
    expect(
      screen.getByText("OpenAI.Codex_26.727.6591.0_arm64__2p2nqsd0c76g0.Msix"),
    ).toBeInTheDocument();
  });

  it("uses a first-install confirmation when there is no current version", async () => {
    const user = userEvent.setup();
    renderPicker("macos", null);

    expect(screen.queryByText("Current")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /26\.727\.51351/ }));

    expect(
      screen.getByText("Download and install 26.727.51351 from GitHub Releases."),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Back" })).toHaveFocus();

    await user.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByRole("button", { name: /26\.727\.51351/ })).toHaveFocus();
  });

  it("returns to the version list when the installed app snapshot changes", async () => {
    const user = userEvent.setup();
    const { rerender } = renderPicker("macos", "26.806.12001", "x64");

    await user.click(screen.getByRole("button", { name: /26\.727\.51351/ }));
    expect(screen.getByRole("heading", { name: "Install 26.727.51351?" })).toBeInTheDocument();

    rerender(
      <I18nProvider>
        <InstallOtherVersionSheet
          open
          platform="macos"
          currentVersion="26.721.81911"
          architecture={null}
          onDismiss={vi.fn()}
        />
      </I18nProvider>,
    );

    expect(screen.getByRole("heading", { name: "Choose install version" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Install 26.727.51351?" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "arm64" })).toHaveFocus();
    expect(screen.getByRole("button", { name: /26\.727\.51351/ })).toBeDisabled();
  });
});
