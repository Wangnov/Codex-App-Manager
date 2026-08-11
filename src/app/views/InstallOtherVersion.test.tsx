import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  HistoricalInstallSelection,
  HistoricalReleaseArchitecture,
  HistoricalReleaseCatalog,
  LocalReleasePackage,
} from "../../shared/types";
import { I18nProvider } from "../i18n";
import { InstallOtherVersionSheet } from "./InstallOtherVersion";

const api = vi.hoisted(() => ({
  historicalReleaseCatalog: vi.fn(),
  historicalPickLocalPackage: vi.fn(),
}));

vi.mock("../../services/managerApi", () => ({
  managerApi: api,
  errorMessage: (cause: unknown) => (cause instanceof Error ? cause.message : String(cause)),
}));

function catalog(
  platform: "macos" | "windows",
  architecture: HistoricalReleaseArchitecture,
): HistoricalReleaseCatalog {
  const release = (version: string, publishedAt: string) => ({
    tag: `codex-app-${version}`,
    version,
    publishedAt,
    assets:
      platform === "macos"
        ? [
            {
              name: `Codex-mac-${architecture}.dmg`,
              size: 560_000_000,
              architecture,
              format: "dmg" as const,
              packageVersion: null,
            },
            {
              name: `Codex-darwin-${architecture}-${version}.zip`,
              size: 530_000_000,
              architecture,
              format: "zip" as const,
              packageVersion: null,
            },
          ]
        : [
            {
              name: `OpenAI.Codex_${
                version === "26.727.51351" ? "26.727.6591.0" : `${version}.0`
              }_${architecture}__2p2nqsd0c76g0.Msix`,
              size: 755_000_000,
              architecture,
              format: "msix" as const,
              packageVersion:
                version === "26.727.51351" ? "26.727.6591.0" : `${version}.0`,
            },
          ],
  });
  return {
    repository: "Wangnov/codex-app-mirror",
    platform,
    architecture,
    releases: [
      release("26.806.12001", "2026-08-06T12:00:00Z"),
      release("26.727.51351", "2026-07-27T12:00:00Z"),
      release("26.721.81911", "2026-07-21T12:00:00Z"),
    ],
  };
}

function localPackage(
  platform: "macos" | "windows",
  architecture: HistoricalReleaseArchitecture,
): LocalReleasePackage {
  const assetName =
    platform === "macos"
      ? `Codex-mac-${architecture}.dmg`
      : `OpenAI.Codex_26.727.6591.0_${architecture}__2p2nqsd0c76g0.Msix`;
  return {
    path: `/Downloads/${assetName}`,
    fileName: assetName,
    size: platform === "macos" ? 560_000_000 : 755_000_000,
    releaseTag: "local-signed-26.727.51351",
    version: "26.727.51351",
    assetName,
    architecture,
    format: platform === "macos" ? "dmg" : "msix",
    packageVersion: platform === "windows" ? "26.727.6591.0" : null,
  };
}

function renderPicker(
  platform: "macos" | "windows",
  currentVersion: string | null = "26.806.12001",
  architecture: string | null = "x64",
  onInstall = vi.fn<(selection: HistoricalInstallSelection, blockUpdates: boolean) => void>(),
) {
  const view = render(
    <I18nProvider>
      <InstallOtherVersionSheet
        open
        platform={platform}
        currentVersion={currentVersion}
        architecture={architecture}
        onDismiss={vi.fn()}
        onInstall={onInstall}
      />
    </I18nProvider>,
  );
  return { ...view, onInstall };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

describe("InstallOtherVersionSheet", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "en");
    api.historicalReleaseCatalog.mockReset();
    api.historicalPickLocalPackage.mockReset();
    api.historicalReleaseCatalog.mockImplementation(
      (platform: "macos" | "windows", architecture: HistoricalReleaseArchitecture) =>
        Promise.resolve(catalog(platform, architecture)),
    );
    api.historicalPickLocalPackage.mockImplementation(
      (platform: "macos" | "windows", architecture: HistoricalReleaseArchitecture) =>
        Promise.resolve(localPackage(platform, architecture)),
    );
  });

  it("loads historical releases and submits the selected GitHub ZIP with updates blocked", async () => {
    const user = userEvent.setup();
    const { onInstall } = renderPicker("macos");

    expect(screen.getByRole("dialog", { name: "Choose install version" })).toBeInTheDocument();
    const target = await screen.findByRole("button", { name: /26\.727\.51351.*GitHub Releases/ });
    expect(api.historicalReleaseCatalog).toHaveBeenCalledWith("macos", "x64");
    expect(screen.getByRole("button", { name: /26\.806\.12001/ })).toBeDisabled();

    await user.click(target);
    expect(screen.getByRole("switch")).toBeChecked();
    await user.click(screen.getByRole("button", { name: /^ZIP/ }));
    await user.click(screen.getByRole("button", { name: "Download and install" }));

    await waitFor(() =>
      expect(onInstall).toHaveBeenCalledWith(
        expect.objectContaining({
          releaseTag: "codex-app-26.727.51351",
          version: "26.727.51351",
          assetName: "Codex-darwin-x64-26.727.51351.zip",
          format: "zip",
          localPath: null,
        }),
        true,
      ),
    );
  });

  it("accepts a locally selected GitHub Release DMG without a Sparkle path", async () => {
    const user = userEvent.setup();
    const { onInstall } = renderPicker("macos", null);

    expect(screen.getByText(".dmg")).toBeInTheDocument();
    expect(screen.getByText(".zip")).toBeInTheDocument();
    expect(screen.queryByText(/sparkle/i)).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Install from a local package/ }));

    expect(await screen.findByText("Codex-mac-x64.dmg")).toBeInTheDocument();
    await user.click(screen.getByRole("switch"));
    await user.click(screen.getByRole("button", { name: "Verify and install" }));
    await waitFor(() =>
      expect(onInstall).toHaveBeenCalledWith(
        expect.objectContaining({
          assetName: "Codex-mac-x64.dmg",
          format: "dmg",
          localPath: "/Downloads/Codex-mac-x64.dmg",
        }),
        false,
      ),
    );
  });

  it("only offers MSIX and preserves the package identity version on Windows", async () => {
    const user = userEvent.setup();
    const { onInstall } = renderPicker("windows", null, "aarch64");

    expect(screen.getByText(".msix")).toBeInTheDocument();
    expect(screen.queryByText(".dmg")).not.toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: /26\.727\.51351/ }));
    expect(screen.getByText("MSIX · arm64 · 26.727.6591.0")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Download and install" }));

    await waitFor(() =>
      expect(onInstall).toHaveBeenCalledWith(
        expect.objectContaining({
          architecture: "arm64",
          format: "msix",
          packageVersion: "26.727.6591.0",
        }),
        true,
      ),
    );
  });

  it("recovers the confirmation sheet when the parent rejects a busy submit", async () => {
    const user = userEvent.setup();
    const onInstall = vi.fn().mockRejectedValue(new Error("Another operation is already running"));
    renderPicker("windows", null, "x64", onInstall);

    await user.click(await screen.findByRole("button", { name: /26\.727\.51351/ }));
    const submit = screen.getByRole("button", { name: "Download and install" });
    await user.click(submit);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Another operation is already running",
    );
    expect(submit).toBeEnabled();
    expect(screen.getByRole("button", { name: "Back" })).toBeEnabled();
  });

  it("recognizes a Windows package-version fallback as the current release", async () => {
    renderPicker("windows", "26.727.6591.0", "x64");

    const current = await screen.findByRole("button", { name: /26\.727\.51351/ });
    expect(current).toBeDisabled();
    expect(current).toHaveTextContent("Current");
    expect(screen.getByRole("button", { name: /26\.806\.12001/ })).toHaveFocus();
  });

  it("requires an explicit architecture before loading the live catalog", async () => {
    const user = userEvent.setup();
    renderPicker("windows", null, null);

    const local = screen.getByRole("button", { name: /Install from a local package/ });
    expect(local).toBeDisabled();
    expect(api.historicalReleaseCatalog).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "arm64" })).toHaveFocus();

    await user.click(screen.getByRole("button", { name: "arm64" }));
    const first = await screen.findByRole("button", { name: /26\.806\.12001/ });
    expect(api.historicalReleaseCatalog).toHaveBeenCalledWith("windows", "arm64");
    await waitFor(() => expect(first).toHaveFocus());
    expect(local).toBeEnabled();
  });

  it("lets an ARM64 host switch to a compatible x64 historical package", async () => {
    const user = userEvent.setup();
    renderPicker("macos", null, "arm64");

    expect(await screen.findByRole("button", { name: /26\.806\.12001/ })).toBeInTheDocument();
    expect(api.historicalReleaseCatalog).toHaveBeenCalledWith("macos", "arm64");
    expect(screen.getByRole("button", { name: "arm64" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    await user.click(screen.getByRole("button", { name: "x64" }));

    await waitFor(() =>
      expect(api.historicalReleaseCatalog).toHaveBeenCalledWith("macos", "x64"),
    );
    expect(screen.getByRole("button", { name: "x64" })).toHaveAttribute("aria-pressed", "true");
    await user.click(screen.getByRole("button", { name: /Install from a local package/ }));
    expect(api.historicalPickLocalPackage).toHaveBeenCalledWith("macos", "x64");
  });

  it("surfaces a catalog error and retries the GitHub request", async () => {
    const user = userEvent.setup();
    api.historicalReleaseCatalog
      .mockRejectedValueOnce(new Error("GitHub API unavailable"))
      .mockResolvedValueOnce(catalog("macos", "x64"));
    renderPicker("macos");

    expect(await screen.findByRole("alert")).toHaveTextContent("GitHub API unavailable");
    await user.click(screen.getByRole("button", { name: "Check again" }));
    expect(await screen.findByRole("button", { name: /26\.727\.51351/ })).toBeInTheDocument();
    expect(api.historicalReleaseCatalog).toHaveBeenCalledTimes(2);
  });

  it("discards a late manual retry after switching package architecture", async () => {
    const user = userEvent.setup();
    const lateArm64Retry = deferred<HistoricalReleaseCatalog>();
    api.historicalReleaseCatalog
      .mockRejectedValueOnce(new Error("GitHub API unavailable"))
      .mockImplementation(
        (platform: "macos" | "windows", architecture: HistoricalReleaseArchitecture) =>
          architecture === "arm64"
            ? lateArm64Retry.promise
            : Promise.resolve(catalog(platform, architecture)),
      );
    renderPicker("macos", null, "arm64");

    expect(await screen.findByRole("alert")).toHaveTextContent("GitHub API unavailable");
    await user.click(screen.getByRole("button", { name: "Check again" }));
    await user.click(screen.getByRole("button", { name: "x64" }));
    expect(await screen.findByRole("button", { name: /26\.727\.51351/ })).toBeInTheDocument();

    await act(async () => {
      lateArm64Retry.resolve(catalog("macos", "arm64"));
      await lateArm64Retry.promise;
    });
    await user.click(screen.getByRole("button", { name: /26\.727\.51351/ }));

    expect(screen.getByRole("group", { name: "DMG · x64" })).toBeInTheDocument();
  });

  it("keeps local package installation available when GitHub is offline", async () => {
    const user = userEvent.setup();
    api.historicalReleaseCatalog.mockRejectedValue(new Error("network offline"));
    renderPicker("macos", null, "x64");

    expect(await screen.findByRole("alert")).toHaveTextContent("network offline");
    await user.click(screen.getByRole("button", { name: /Install from a local package/ }));
    expect(await screen.findByText("Codex-mac-x64.dmg")).toBeInTheDocument();
    expect(api.historicalPickLocalPackage).toHaveBeenCalledWith("macos", "x64");
    expect(screen.getByRole("button", { name: "Verify and install" })).toBeEnabled();
  });

  it("returns to a clean architecture choice when the install snapshot changes", async () => {
    const user = userEvent.setup();
    const onInstall = vi.fn();
    const { rerender } = renderPicker("macos", "26.806.12001", "x64", onInstall);

    await user.click(await screen.findByRole("button", { name: /26\.727\.51351/ }));
    expect(screen.getByRole("heading", { name: "Install 26.727.51351?" })).toBeInTheDocument();

    rerender(
      <I18nProvider>
        <InstallOtherVersionSheet
          open
          platform="macos"
          currentVersion="26.721.81911"
          architecture={null}
          onDismiss={vi.fn()}
          onInstall={onInstall}
        />
      </I18nProvider>,
    );

    expect(screen.getByRole("heading", { name: "Choose install version" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Install 26.727.51351?" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "arm64" })).toHaveFocus();
    expect(api.historicalReleaseCatalog).toHaveBeenCalledTimes(1);
  });
});
