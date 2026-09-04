import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { listen } from "@tauri-apps/api/event";

import { managerApi } from "../../services/managerApi";
import type {
  AppSettings,
  CapabilityCheck,
  DownloadProgress,
  InstalledWindowsCodex,
  OperationSnapshot,
  OperationCompletion,
  WinCapabilityReport,
  WindowsUpdatePlan,
  WinInstallStatus,
  WinPerformReport,
  WinUpdateReport,
} from "../../shared/types";
import { DEFAULT_SETTINGS, emptyOperationOutcome } from "../../shared/types";
import { I18nProvider } from "../i18n";
import { ThemeProvider } from "../theme";
import { useManagerUpdatePrompt } from "../ManagerUpdatePrompt";
import { WinHome } from "./WinHome";

vi.mock("../motion", () => ({ useHomeMotion: () => {} }));

vi.mock("../../services/managerApi", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("../../services/managerApi")>();
  return {
    ...actual,
    managerApi: {
      beginTrackedOperation: vi.fn(),
      armDestructive: vi.fn(),
      checkManagerUpdate: vi.fn(),
      getSettings: vi.fn(),
      getSettingsStrict: vi.fn(),
      setSettings: vi.fn(),
      getOperationSnapshot: vi.fn(() => Promise.resolve(null)),
      getPausedOperationSnapshot: vi.fn(() => Promise.resolve(null)),
      getOperationCompletion: vi.fn(() => Promise.resolve(null)),
      historicalReleaseCatalog: vi.fn(
        (platform: "macos" | "windows", architecture: "arm64" | "x64") =>
          Promise.resolve({
            repository: "Wangnov/codex-app-mirror",
            platform,
            architecture,
            releases: [],
          }),
      ),
      historicalPickLocalPackage: vi.fn(),
      winStatus: vi.fn(),
      winPlanUpdate: vi.fn(),
      winPerformUpdate: vi.fn(),
      winInstallHistoricalRelease: vi.fn(),
      winAdopt: vi.fn(),
      winAdoptPath: vi.fn(),
      winLaunch: vi.fn(),
      winPauseDownload: vi.fn(),
      winCancelDownload: vi.fn(),
      winDiscardDownload: vi.fn(),
      winPickExistingInstall: vi.fn(),
      winPickInstallDir: vi.fn(),
      winDefaultInstallRoot: vi.fn(),
    },
  };
});

const api = vi.mocked(managerApi);
const listenMock = vi.mocked(listen);

const ok: CapabilityCheck = { state: "available", detail: "" };
const unknown: CapabilityCheck = { state: "unknown", detail: "" };

const CAPS_OK: WinCapabilityReport = {
  addAppxPackage: ok,
  appxService: ok,
  sideloadPolicy: ok,
  appInstaller: ok,
  msixDeployment: ok,
  meteredNetwork: ok,
  recommendation: "msix-preferred",
  notes: [],
};

const INSTALLED: InstalledWindowsCodex = {
  path: "C:\\Program Files\\WindowsApps\\Codex",
  version: "1.0.0",
  arch: "x64",
  source: "msix",
  packageFamilyName: "OpenAI.Codex_x",
};

const PLAN_UPDATE: WindowsUpdatePlan = {
  upToDate: false,
  currentVersion: "1.0.0",
  latestVersion: "2.0.0",
  packageMoniker: "Codex_2.0.0_x64",
  packageUrl: "https://example.invalid/codex.msix",
  downloadSize: 2048,
  sha256: "deadbeef",
  route: "msix-sideload",
  portableFallbackReady: false,
  warnings: [],
};

function report(overrides: Partial<WinUpdateReport> = {}): WinUpdateReport {
  return {
    manifestUrl: "m",
    checksumsUrl: "c",
    packageUrl: "p",
    release: {
      version: "2.0.0",
      packageVersion: "2.0.0.0",
      packageMoniker: "Codex_2.0.0_x64",
      architecture: "x64",
      contentLength: 2048,
      etag: null,
      storeProductId: null,
      packageIdentity: null,
    },
    installed: INSTALLED,
    capabilities: CAPS_OK,
    plan: PLAN_UPDATE,
    ...overrides,
  };
}

const STATUS_MANAGED: WinInstallStatus = {
  installed: INSTALLED,
  status: "managed",
};
const ACTIVE_OPERATION: OperationSnapshot = {
  id: "op-active",
  kind: "update",
  phase: "downloading",
  progress: { downloaded: 10, total: 100, source: "mirror.example" },
  paused: false,
  cancellable: true,
  interruptible: true,
};

const PERFORM_OK: WinPerformReport = {
  success: true,
  action: "msix-sideload",
  message: "updated",
  stage: {
    upToDate: false,
    route: "msix-sideload",
    latestVersion: "2.0.0",
    packageMoniker: "Codex_2.0.0_x64",
    downloadSize: 2048,
    stagedPath: "s",
    sha256: "deadbeef",
    hashVerified: true,
    authenticode: null,
    identity: null,
    identityVerified: true,
    installReady: true,
    portableFallbackReady: false,
    notes: [],
  },
  sideload: null,
  portable: null,
  msixHealth: null,
  installed: INSTALLED,
  fallbackAvailable: false,
  fallbackAttempted: false,
  notes: [],
  outcome: emptyOperationOutcome({
    primaryOk: true,
    appState: "present",
    installClass: "managed",
  }),
};

function settings(overrides: Partial<AppSettings> = {}): AppSettings {
  return { ...DEFAULT_SETTINGS, ...overrides };
}

function WinHomeWithManagerPrompt() {
  const managerUpdatePrompt = useManagerUpdatePrompt();
  return <WinHome onOpenSettings={vi.fn()} managerUpdatePrompt={managerUpdatePrompt} />;
}

function renderWinHome(withManagerPrompt = false) {
  return render(
    <ThemeProvider>
      <I18nProvider>
        {withManagerPrompt ? <WinHomeWithManagerPrompt /> : <WinHome onOpenSettings={vi.fn()} />}
      </I18nProvider>
    </ThemeProvider>,
  );
}

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("WinHome state machine", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "zh-CN");
    sessionStorage.clear();
    api.beginTrackedOperation.mockResolvedValue("win-install-op-1");
    api.armDestructive.mockResolvedValue("win-op-1");
    api.checkManagerUpdate.mockResolvedValue({ kind: "none" });
    api.getOperationCompletion.mockResolvedValue(null);
    api.getSettings.mockResolvedValue(settings());
    api.getSettingsStrict.mockResolvedValue(settings());
    api.winDefaultInstallRoot.mockResolvedValue(DEFAULT_SETTINGS.installRoot);
    api.winStatus.mockResolvedValue(STATUS_MANAGED);
    api.winPlanUpdate.mockResolvedValue(report());
    api.winPerformUpdate.mockResolvedValue(PERFORM_OK);
    api.winPauseDownload.mockResolvedValue(true);
    api.winCancelDownload.mockResolvedValue(true);
    api.winDiscardDownload.mockResolvedValue(undefined);
    api.getOperationSnapshot.mockResolvedValue(null);
    api.getPausedOperationSnapshot.mockResolvedValue(null);
    api.historicalReleaseCatalog.mockImplementation((platform, architecture) =>
      Promise.resolve({
        repository: "Wangnov/codex-app-mirror",
        platform,
        architecture,
        releases: [],
      }),
    );
    api.historicalPickLocalPackage.mockResolvedValue(null);
  });

  it("offers install when nothing is detected", async () => {
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.winStatus.mockResolvedValue({ installed: null, status: "none" });
    renderWinHome();
    expect(
      await screen.findByRole("button", { name: /安装 Codex/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /选择安装版本/ }),
    ).toBeInTheDocument();
  });

  it("manually checks Codex and Manager with automatic checks disabled", async () => {
    const user = userEvent.setup();
    const manualOnly = settings({ checkOnStartup: false, periodicCheck: false });
    api.getSettings.mockResolvedValue(manualOnly);
    api.getSettingsStrict.mockResolvedValue(manualOnly);
    api.checkManagerUpdate.mockResolvedValue({
      kind: "available", version: "0.5.5", currentVersion: "0.5.4",
      installAndRelaunch: vi.fn(), discard: vi.fn(),
    });
    renderWinHome(true);
    const recheck = await screen.findByRole("button", { name: "重新检查" });
    expect(api.winPlanUpdate).not.toHaveBeenCalled();
    expect(api.checkManagerUpdate).not.toHaveBeenCalled();
    await user.click(recheck);
    expect(await screen.findByText("发现管理器新版本 0.5.5")).toBeInTheDocument();
    expect(await screen.findByText("有新版本", { selector: ".headline" })).toBeInTheDocument();
    expect(api.winPlanUpdate).toHaveBeenCalledTimes(1);
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(1);
  });

  it("finds a Manager update even when the parallel Codex check fails", async () => {
    const user = userEvent.setup();
    renderWinHome(true);
    await screen.findByRole("button", { name: "重新检查" });
    api.winPlanUpdate.mockRejectedValue(new Error("Codex feed offline"));
    api.checkManagerUpdate.mockResolvedValue({
      kind: "available", version: "0.5.5", currentVersion: "0.5.4",
      installAndRelaunch: vi.fn(), discard: vi.fn(),
    });
    await user.click(screen.getByRole("button", { name: "重新检查" }));
    expect(await screen.findByText("发现管理器新版本 0.5.5")).toBeInTheDocument();
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(2);
    expect(api.winPlanUpdate).toHaveBeenCalledTimes(2);
  });

  it("does not let a failed Manager check hide a successful Codex check", async () => {
    const user = userEvent.setup();
    renderWinHome(true);
    await screen.findByRole("button", { name: "重新检查" });
    api.checkManagerUpdate.mockResolvedValue({ kind: "unavailable" });
    await user.click(screen.getByRole("button", { name: "重新检查" }));
    expect(await screen.findByText("有新版本", { selector: ".headline" })).toBeInTheDocument();
    expect(api.checkManagerUpdate).toHaveBeenCalledTimes(2);
    expect(api.winPlanUpdate).toHaveBeenCalledTimes(2);
  });

  it("signs the perform expectation from the SAME report snapshot it shows", async () => {
    const user = userEvent.setup();
    renderWinHome();
    await user.click(await screen.findByRole("button", { name: /立即更新/ }));
    await user.click(await screen.findByRole("button", { name: "更新" }));
    await waitFor(() =>
      expect(api.winPerformUpdate).toHaveBeenCalledWith(
        true,
        {
          currentVersion: "1.0.0",
          latestVersion: "2.0.0",
          packageMoniker: "Codex_2.0.0_x64",
          route: "msix-sideload",
        },
        undefined,
        "win-op-1",
        "perform",
      ),
    );
  });

  it("resumes an ordinary first install with its confirmed one-shot root", async () => {
    api.getPausedOperationSnapshot.mockResolvedValue({
      id: "ordinary-win-pause",
      kind: "update",
      phase: "downloading",
      progress: {
        downloaded: 1024,
        total: 2048,
        source: "github.com",
      },
      paused: true,
      cancellable: true,
      interruptible: true,
      resume: {
        kind: "install",
        installRoot: "D:\\Selected\\Codex",
        expectation: {
          platform: "windows",
          currentVersion: null,
          targetVersion: "1.5.0",
          packageMoniker: "Codex_1.5.0_x64",
          route: "portable-fallback",
        },
      },
    });
    api.armDestructive.mockResolvedValueOnce("resumed-ordinary-win");

    const user = userEvent.setup();
    renderWinHome();
    expect(await screen.findByText("下载已暂停")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() =>
      expect(api.winPerformUpdate).toHaveBeenCalledWith(
        true,
        {
          currentVersion: null,
          latestVersion: "1.5.0",
          packageMoniker: "Codex_1.5.0_x64",
          route: "portable-fallback",
        },
        "D:\\Selected\\Codex",
        "resumed-ordinary-win",
        "install",
      ),
    );
  });

  it("routes a selected GitHub MSIX through the tracked downgrade installer", async () => {
    api.historicalReleaseCatalog.mockResolvedValue({
      repository: "Wangnov/codex-app-mirror",
      platform: "windows",
      architecture: "x64",
      releases: [
        {
          tag: "codex-app-0.9.0",
          version: "0.9.0",
          publishedAt: "2026-01-01T00:00:00Z",
          assets: [
            {
              name: "OpenAI.Codex_0.9.0.0_x64__2p2nqsd0c76g0.Msix",
              size: 2048,
              architecture: "x64",
              format: "msix",
              packageVersion: "0.9.0.0",
            },
          ],
        },
      ],
    });
    api.winInstallHistoricalRelease.mockResolvedValue({
      ...PERFORM_OK,
      installed: { ...INSTALLED, version: "0.9.0" },
      stage: {
        ...PERFORM_OK.stage,
        latestVersion: "0.9.0",
        packageMoniker: "OpenAI.Codex_0.9.0.0_x64__2p2nqsd0c76g0",
      },
    });
    const user = userEvent.setup();
    renderWinHome();

    await user.click(
      await screen.findByRole("button", { name: /选择安装版本/ }),
    );
    await user.click(await screen.findByRole("button", { name: /0\.9\.0/ }));
    await user.click(screen.getByRole("button", { name: /下载并安装/ }));

    await waitFor(() =>
      expect(api.winInstallHistoricalRelease).toHaveBeenCalledWith(
        expect.objectContaining({
          releaseTag: "codex-app-0.9.0",
          assetName: "OpenAI.Codex_0.9.0.0_x64__2p2nqsd0c76g0.Msix",
          format: "msix",
          packageVersion: "0.9.0.0",
        }),
        true,
        {
          currentPath: INSTALLED.path,
          currentVersion: INSTALLED.version,
          currentSource: INSTALLED.source,
        },
        "win-op-1",
        undefined,
      ),
    );
    expect(api.armDestructive).toHaveBeenCalledWith("update");
  });

  it("collects a fallback destination and uses an install token for a fresh historical MSIX", async () => {
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.winStatus.mockResolvedValue({ installed: null, status: "none" });
    api.historicalReleaseCatalog.mockResolvedValue({
      repository: "Wangnov/codex-app-mirror",
      platform: "windows",
      architecture: "x64",
      releases: [
        {
          tag: "codex-app-0.9.0",
          version: "0.9.0",
          publishedAt: "2026-01-01T00:00:00Z",
          assets: [
            {
              name: "OpenAI.Codex_0.9.0.0_x64__2p2nqsd0c76g0.Msix",
              size: 2048,
              architecture: "x64",
              format: "msix",
              packageVersion: "0.9.0.0",
            },
          ],
        },
      ],
    });
    api.winInstallHistoricalRelease.mockResolvedValue({
      ...PERFORM_OK,
      installed: { ...INSTALLED, version: "0.9.0" },
    });
    const user = userEvent.setup();
    renderWinHome();

    await user.click(
      await screen.findByRole("button", { name: /选择安装版本/ }),
    );
    await user.click(screen.getByRole("button", { name: "x64" }));
    await user.click(await screen.findByRole("button", { name: /0\.9\.0/ }));
    await user.click(screen.getByRole("button", { name: /下载并安装/ }));

    expect(await screen.findByText("选择安装位置")).toBeInTheDocument();
    expect(api.winInstallHistoricalRelease).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: /安装到默认位置/ }));

    await waitFor(() =>
      expect(api.beginTrackedOperation).toHaveBeenCalledWith("install"),
    );
    expect(api.armDestructive).not.toHaveBeenCalledWith("install");
    expect(api.winInstallHistoricalRelease).toHaveBeenCalledWith(
      expect.objectContaining({ releaseTag: "codex-app-0.9.0" }),
      true,
      { currentPath: null, currentVersion: null, currentSource: null },
      "win-install-op-1",
      DEFAULT_SETTINGS.installRoot,
    );
  });

  it("keeps a paused historical install and its original expectation when lease acquisition fails", async () => {
    const confirmedExpectation = {
      platform: "windows" as const,
      currentPath: "D:\\Confirmed\\Codex",
      currentVersion: "0.8.0",
      currentSource: "portable",
    };
    const selection = {
      releaseTag: "codex-app-0.9.0",
      version: "0.9.0",
      assetName: "OpenAI.Codex_0.9.0.0_x64__2p2nqsd0c76g0.Msix",
      architecture: "x64" as const,
      format: "msix" as const,
      packageVersion: "0.9.0.0",
      localPath: null,
      localFileName: null,
    };
    api.getPausedOperationSnapshot.mockResolvedValue({
      id: "historical-win-pause",
      kind: "update",
      phase: "downloading",
      progress: {
        downloaded: 1024,
        total: 2048,
        source: "github.com",
        operationId: "historical-win-pause",
      },
      paused: true,
      cancellable: true,
      interruptible: true,
      historical: {
        selection,
        blockUpdates: true,
        expectation: confirmedExpectation,
        installRoot: "D:\\Fallback\\Codex",
      },
    });
    api.winStatus.mockResolvedValue(STATUS_MANAGED);
    let rejectLease: ((cause: unknown) => void) | undefined;
    api.armDestructive
      .mockImplementationOnce(
        () =>
          new Promise((_resolve, reject) => {
            rejectLease = reject;
          }),
      )
      .mockResolvedValueOnce("resumed-historical-win");
    api.winInstallHistoricalRelease.mockResolvedValue({
      ...PERFORM_OK,
      installed: { ...INSTALLED, version: "0.9.0" },
    });

    const user = userEvent.setup();
    renderWinHome();
    expect(await screen.findByText("下载已暂停")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(rejectLease).toBeDefined());
    expect(
      screen.queryByRole("button", { name: "继续" }),
    ).not.toBeInTheDocument();
    expect(api.winDiscardDownload).not.toHaveBeenCalled();
    await act(async () => {
      rejectLease?.(new Error("another operation owns the lease"));
      await Promise.resolve();
    });
    expect(await screen.findByRole("alert")).toHaveTextContent("操作未完成");
    expect(screen.getByText("下载已暂停")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "继续" })).toBeEnabled();

    await user.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() =>
      expect(api.winInstallHistoricalRelease).toHaveBeenCalledWith(
        selection,
        true,
        {
          currentPath: confirmedExpectation.currentPath,
          currentVersion: confirmedExpectation.currentVersion,
          currentSource: confirmedExpectation.currentSource,
        },
        "resumed-historical-win",
        "D:\\Fallback\\Codex",
      ),
    );
  });

  it("asks for a destination before a fresh portable historical install", async () => {
    api.getSettings.mockResolvedValue(
      settings({ checkOnStartup: false, windowsInstallMode: "portable" }),
    );
    api.winStatus.mockResolvedValue({ installed: null, status: "none" });
    api.historicalReleaseCatalog.mockResolvedValue({
      repository: "Wangnov/codex-app-mirror",
      platform: "windows",
      architecture: "x64",
      releases: [
        {
          tag: "codex-app-0.9.0",
          version: "0.9.0",
          publishedAt: "2026-01-01T00:00:00Z",
          assets: [
            {
              name: "OpenAI.Codex_0.9.0.0_x64__2p2nqsd0c76g0.Msix",
              size: 2048,
              architecture: "x64",
              format: "msix",
              packageVersion: "0.9.0.0",
            },
          ],
        },
      ],
    });
    api.winInstallHistoricalRelease.mockResolvedValue({
      ...PERFORM_OK,
      installed: { ...INSTALLED, version: "0.9.0" },
    });
    const user = userEvent.setup();
    renderWinHome();

    await user.click(
      await screen.findByRole("button", { name: /选择安装版本/ }),
    );
    await user.click(screen.getByRole("button", { name: "x64" }));
    await user.click(await screen.findByRole("button", { name: /0\.9\.0/ }));
    await user.click(screen.getByRole("button", { name: /下载并安装/ }));

    expect(await screen.findByText("选择安装位置")).toBeInTheDocument();
    expect(api.winInstallHistoricalRelease).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: /安装到默认位置/ }));
    await waitFor(() =>
      expect(api.winInstallHistoricalRelease).toHaveBeenCalledWith(
        expect.objectContaining({ releaseTag: "codex-app-0.9.0" }),
        true,
        { currentPath: null, currentVersion: null, currentSource: null },
        "win-install-op-1",
        DEFAULT_SETTINGS.installRoot,
      ),
    );
  });

  it("installs a local MSIX without consulting the failed online plan again", async () => {
    api.winStatus.mockResolvedValue({ installed: null, status: "none" });
    api.winPlanUpdate.mockRejectedValue(new Error("manifest unreachable"));
    api.historicalReleaseCatalog.mockRejectedValue(
      new Error("github unreachable"),
    );
    const assetName = "OpenAI.Codex_26.727.6591.0_x64__2p2nqsd0c76g0.Msix";
    api.historicalPickLocalPackage.mockResolvedValue({
      path: `C:\\Downloads\\${assetName}`,
      fileName: assetName,
      size: 755_000_000,
      releaseTag: "local-signed-26.727.51351",
      version: "26.727.51351",
      assetName,
      architecture: "x64",
      format: "msix",
      packageVersion: "26.727.6591.0",
    });
    api.winInstallHistoricalRelease.mockResolvedValue({
      ...PERFORM_OK,
      installed: { ...INSTALLED, version: "26.727.51351" },
    });
    const user = userEvent.setup();
    renderWinHome();

    expect(await screen.findByText("检查失败")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /选择安装版本/ }));
    await user.click(screen.getByRole("button", { name: "x64" }));
    expect(
      await screen.findByRole("button", { name: /从本地安装包安装/ }),
    ).toBeEnabled();
    expect(api.winPlanUpdate).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: /从本地安装包安装/ }));
    await user.click(await screen.findByRole("button", { name: /验证并安装/ }));

    expect(await screen.findByText("选择安装位置")).toBeInTheDocument();
    expect(api.winPlanUpdate).toHaveBeenCalledTimes(1);
    expect(api.winInstallHistoricalRelease).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: /安装到默认位置/ }));
    await waitFor(() =>
      expect(api.winInstallHistoricalRelease).toHaveBeenCalledWith(
        expect.objectContaining({
          releaseTag: "local-signed-26.727.51351",
          localPath: `C:\\Downloads\\${assetName}`,
        }),
        true,
        { currentPath: null, currentVersion: null, currentSource: null },
        "win-install-op-1",
        DEFAULT_SETTINGS.installRoot,
      ),
    );
  });

  it("settles on up-to-date", async () => {
    api.winPlanUpdate.mockResolvedValue(
      report({
        plan: { ...PLAN_UPDATE, upToDate: true, latestVersion: "1.0.0" },
      }),
    );
    renderWinHome();
    expect(
      await screen.findByText("已是最新", { selector: ".headline" }),
    ).toBeInTheDocument();
  });

  it("gates an external install behind adopt", async () => {
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.winStatus.mockResolvedValue({
      installed: INSTALLED,
      status: "external",
    });
    api.winAdopt.mockResolvedValue(STATUS_MANAGED);
    const user = userEvent.setup();
    renderWinHome();
    const adopt = await screen.findByRole("button", { name: /开始管理/ });
    expect(
      screen.queryByRole("button", { name: /选择安装版本/ }),
    ).not.toBeInTheDocument();
    await user.click(adopt);
    await waitFor(() => expect(api.winAdopt).toHaveBeenCalledTimes(1));
  });

  it("keeps partial-install guidance aligned with the rendered recovery action", async () => {
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.winStatus
      .mockResolvedValueOnce({ installed: null, status: "none" })
      .mockResolvedValue({ installed: INSTALLED, status: "external" });
    api.winPlanUpdate
      .mockResolvedValueOnce(report({ installed: null }))
      .mockResolvedValue(
        report({
          plan: { ...PLAN_UPDATE, upToDate: true, latestVersion: "1.0.0" },
        }),
      );
    api.winPerformUpdate.mockResolvedValue({
      ...PERFORM_OK,
      message: "installed; managed record failed",
      outcome: emptyOperationOutcome({
        primaryOk: true,
        appState: "present",
        installClass: "external",
        provenance: { state: "failed", detail: "write failed" },
        recoveryActions: ["record_provenance"],
      }),
    });

    const user = userEvent.setup();
    renderWinHome();
    await user.click(await screen.findByRole("button", { name: /安装 Codex/ }));

    expect(
      await screen.findByText(/请点「开始管理」，勿重复安装/),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "开始管理" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("已安装 Codex", { selector: ".rb-title" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("installed; managed record failed", {
        selector: ".rb-detail",
      }),
    ).not.toBeInTheDocument();
    const diagnostics = screen
      .getByText("installed; managed record failed", {
        selector: ".errdetails",
      })
      .closest("details");
    expect(diagnostics).not.toHaveAttribute("open");
  });

  it("keeps English backend prose collapsed in a non-English partial-install UI", async () => {
    localStorage.setItem("cam.lang", "fr");
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.winStatus
      .mockResolvedValueOnce({ installed: null, status: "none" })
      .mockResolvedValue({ installed: INSTALLED, status: "external" });
    api.winPlanUpdate
      .mockResolvedValueOnce(report({ installed: null }))
      .mockResolvedValue(
        report({
          plan: { ...PLAN_UPDATE, upToDate: true, latestVersion: "1.0.0" },
        }),
      );
    api.winPerformUpdate.mockResolvedValue({
      ...PERFORM_OK,
      message: "installed; managed record failed",
      outcome: emptyOperationOutcome({
        primaryOk: true,
        appState: "present",
        installClass: "external",
        provenance: { state: "failed", detail: "write failed" },
        recoveryActions: ["record_provenance"],
      }),
    });

    const user = userEvent.setup();
    renderWinHome();
    await user.click(
      await screen.findByRole("button", { name: /Installer Codex/ }),
    );

    expect(
      screen.getByText("Codex installé", { selector: ".rb-title" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("installed; managed record failed", {
        selector: ".rb-detail",
      }),
    ).not.toBeInTheDocument();
    const diagnostics = screen
      .getByText("installed; managed record failed", {
        selector: ".errdetails",
      })
      .closest("details");
    expect(diagnostics).not.toHaveAttribute("open");
  });

  it("does not let a pre-update managed snapshot clear a new partial-outcome guard", async () => {
    api.getSettings.mockResolvedValue(settings({ askBefore: false }));
    api.winStatus
      .mockResolvedValueOnce(STATUS_MANAGED)
      .mockResolvedValue({ installed: INSTALLED, status: "external" });
    api.winPlanUpdate.mockResolvedValueOnce(report()).mockResolvedValue(
      report({
        plan: { ...PLAN_UPDATE, upToDate: true, latestVersion: "2.0.0" },
      }),
    );
    api.winPerformUpdate.mockResolvedValue({
      ...PERFORM_OK,
      message: "updated; managed record failed",
      outcome: emptyOperationOutcome({
        primaryOk: true,
        appState: "present",
        installClass: "external",
        provenance: { state: "failed", detail: "write failed" },
        recoveryActions: ["record_provenance"],
      }),
    });

    const user = userEvent.setup();
    renderWinHome();
    await user.click(await screen.findByRole("button", { name: /立即更新/ }));

    expect(
      await screen.findByText(/请点「开始管理」，勿重复安装/),
    ).toBeInTheDocument();
    expect(sessionStorage.getItem("cam.win.provenance-recovery")).toContain(
      "win-op-1",
    );
  });

  it("requires re-detection instead of offering reinstall when a partial install is not found", async () => {
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.winStatus
      .mockResolvedValueOnce({ installed: null, status: "none" })
      .mockResolvedValueOnce({ installed: null, status: "none" })
      .mockResolvedValue({ installed: INSTALLED, status: "external" });
    api.winPlanUpdate
      .mockResolvedValueOnce(report({ installed: null }))
      .mockResolvedValueOnce(report({ installed: null }))
      .mockResolvedValue(
        report({
          plan: { ...PLAN_UPDATE, upToDate: true, latestVersion: "1.0.0" },
        }),
      );
    api.winPerformUpdate.mockResolvedValue({
      ...PERFORM_OK,
      installed: null,
      message: "installed but not detected for managed record",
      outcome: emptyOperationOutcome({
        primaryOk: true,
        appState: "unknown",
        installClass: null,
        provenance: { state: "failed", detail: "install not detected" },
        recoveryActions: ["record_provenance"],
      }),
    });

    const user = userEvent.setup();
    renderWinHome();
    await user.click(await screen.findByRole("button", { name: /安装 Codex/ }));

    expect(
      await screen.findByText(/暂时无法确认 Codex 是否已写入磁盘/),
    ).toHaveTextContent("请点「重新检查」重新检测；确认前请勿重复安装");
    expect(
      screen.queryByRole("button", { name: /安装 Codex/ }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "重新检查" }));

    expect(
      await screen.findByText(/请点「开始管理」，勿重复安装/),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "开始管理" }),
    ).toBeInTheDocument();
  });

  it("keeps the partial-install recovery lock across a renderer reload", async () => {
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.winStatus.mockResolvedValue({ installed: null, status: "none" });
    api.winPlanUpdate.mockResolvedValue(report({ installed: null }));
    api.winPerformUpdate.mockReturnValue(
      new Promise<WinPerformReport>(() => {}),
    );

    const user = userEvent.setup();
    const firstRenderer = renderWinHome();
    await user.click(await screen.findByRole("button", { name: /安装 Codex/ }));
    await waitFor(() => expect(api.winPerformUpdate).toHaveBeenCalledTimes(1));

    firstRenderer.unmount();
    renderWinHome();

    expect(
      await screen.findByText(/暂时无法确认 Codex 是否已写入磁盘/),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "重新检查" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /安装 Codex/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /选择安装版本/ }),
    ).not.toBeInTheDocument();
  });

  it("keeps reinstall blocked when invoke rejects after commit with an unknown outcome", async () => {
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.winStatus.mockResolvedValue({ installed: null, status: "none" });
    api.winPlanUpdate.mockResolvedValue(report({ installed: null }));
    api.getOperationCompletion.mockResolvedValue({
      id: "win-op-1",
      kind: "update",
      phase: "finishing",
      state: "outcome-unknown",
    });
    api.winPerformUpdate.mockRejectedValue({
      code: "internal_error",
      message: "install-root settings save failed after install",
    });

    const user = userEvent.setup();
    renderWinHome();
    await user.click(await screen.findByRole("button", { name: /安装 Codex/ }));

    expect(
      await screen.findByText(/暂时无法确认 Codex 是否已写入磁盘/),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /安装 Codex/ }),
    ).not.toBeInTheDocument();
    expect(sessionStorage.getItem("cam.win.provenance-recovery")).toContain(
      "win-op-1",
    );
  });

  it("releases the guard after a backend-proven pre-commit failure and allows retry", async () => {
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.winStatus.mockResolvedValue({ installed: null, status: "none" });
    api.winPlanUpdate.mockResolvedValue(report({ installed: null }));
    api.getOperationCompletion.mockResolvedValue({
      id: "win-op-1",
      kind: "update",
      phase: "downloading",
      state: "failed-before-commit",
    });
    api.winPerformUpdate
      .mockRejectedValueOnce({
        code: "network_error",
        message: "download failed",
      })
      .mockResolvedValueOnce(PERFORM_OK);

    const user = userEvent.setup();
    renderWinHome();
    await user.click(await screen.findByRole("button", { name: /安装 Codex/ }));

    const retry = await screen.findByRole("button", { name: /安装 Codex/ });
    expect(sessionStorage.getItem("cam.win.provenance-recovery")).toBeNull();
    await user.click(retry);
    await waitFor(() => expect(api.winPerformUpdate).toHaveBeenCalledTimes(2));
  });

  it("releases the guard after a backend-proven rollback and allows retry", async () => {
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.winStatus.mockResolvedValue({ installed: null, status: "none" });
    api.winPlanUpdate.mockResolvedValue(report({ installed: null }));
    api.getOperationCompletion.mockResolvedValue({
      id: "win-op-1",
      kind: "update",
      phase: "finishing",
      state: "rolled-back",
    });
    api.winPerformUpdate
      .mockRejectedValueOnce({
        code: "internal_error",
        message: "portable health check failed; absent state restored",
      })
      .mockResolvedValueOnce(PERFORM_OK);

    const user = userEvent.setup();
    renderWinHome();
    await user.click(await screen.findByRole("button", { name: /安装 Codex/ }));

    const retry = await screen.findByRole("button", { name: /安装 Codex/ });
    expect(sessionStorage.getItem("cam.win.provenance-recovery")).toBeNull();
    await user.click(retry);
    await waitFor(() => expect(api.winPerformUpdate).toHaveBeenCalledTimes(2));
  });

  it("releases a reloaded guard after backend proves the install failed before commit", async () => {
    sessionStorage.setItem(
      "cam.win.provenance-recovery",
      JSON.stringify({ state: "unknown", token: "failed-op" }),
    );
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.winStatus.mockResolvedValue({ installed: null, status: "none" });
    api.winPlanUpdate.mockResolvedValue(report({ installed: null }));
    api.getOperationCompletion.mockImplementation((token) =>
      Promise.resolve(
        token === "failed-op"
          ? {
              id: "failed-op",
              kind: "update",
              phase: "downloading",
              state: "failed-before-commit",
            }
          : null,
      ),
    );
    api.armDestructive.mockResolvedValue("retry-op");

    const user = userEvent.setup();
    renderWinHome();
    const install = await screen.findByRole("button", { name: /安装 Codex/ });
    await user.click(install);

    await waitFor(() => expect(api.winPerformUpdate).toHaveBeenCalled());
  });

  it("does not let an old reconciliation clear a newer token", async () => {
    const oldCompletion = deferred<OperationCompletion | null>();
    const newCompletion = deferred<OperationCompletion | null>();
    const oldStatusProbe = deferred<WinInstallStatus>();
    sessionStorage.setItem(
      "cam.win.provenance-recovery",
      JSON.stringify({ state: "unknown", token: "old-op" }),
    );
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.winStatus
      .mockResolvedValueOnce({ installed: null, status: "none" })
      .mockReturnValueOnce(oldStatusProbe.promise)
      .mockResolvedValue({ installed: null, status: "none" });
    api.winPlanUpdate.mockResolvedValue(report({ installed: null }));
    api.getOperationCompletion.mockImplementation((token) =>
      token === "old-op" ? oldCompletion.promise : newCompletion.promise,
    );

    renderWinHome();
    await waitFor(() =>
      expect(api.getOperationCompletion).toHaveBeenCalledWith("old-op"),
    );

    sessionStorage.setItem(
      "cam.win.provenance-recovery",
      JSON.stringify({ state: "unknown", token: "new-op" }),
    );
    await act(async () => {
      oldCompletion.resolve({
        id: "old-op",
        kind: "update",
        phase: "downloading",
        state: "failed-before-commit",
      });
      await oldCompletion.promise;
    });

    // Clearing old-op adopts the replacement marker into React state. Keep its
    // reconciliation pending while the old status probe and old finally settle:
    // neither is allowed to release the newer generation's busy state.
    await waitFor(() =>
      expect(api.getOperationCompletion).toHaveBeenCalledWith("new-op"),
    );
    await act(async () => {
      oldStatusProbe.resolve({ installed: null, status: "none" });
      await oldStatusProbe.promise;
    });

    await waitFor(() =>
      expect(sessionStorage.getItem("cam.win.provenance-recovery")).toContain(
        "new-op",
      ),
    );
    expect(
      screen.getByText("正在检查…", { selector: ".headline" }),
    ).toBeInTheDocument();

    await act(async () => {
      newCompletion.resolve(null);
      await newCompletion.promise;
    });
    expect(
      await screen.findByText(/暂时无法确认 Codex 是否已写入磁盘/),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /安装 Codex/ }),
    ).not.toBeInTheDocument();
  });

  it("keeps recovery busy until both status and plan probes settle", async () => {
    const statusProbe = deferred<WinInstallStatus>();
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.winStatus
      .mockResolvedValueOnce({ installed: null, status: "none" })
      .mockReturnValueOnce(statusProbe.promise);
    api.winPlanUpdate.mockResolvedValue(report({ installed: null }));
    api.getOperationCompletion.mockResolvedValue({
      id: "win-op-1",
      kind: "update",
      phase: "finishing",
      state: "outcome-unknown",
    });
    api.winPerformUpdate.mockRejectedValue({
      code: "internal_error",
      message: "invoke returned after commit",
    });

    const user = userEvent.setup();
    renderWinHome();
    await user.click(await screen.findByRole("button", { name: /安装 Codex/ }));
    // Target freezing adds one plan read before the existing recovery probes.
    await waitFor(() => expect(api.winPlanUpdate).toHaveBeenCalledTimes(3));

    // The plan probe has completed, but the status probe still owns this
    // recovery generation. Reinstall must remain unavailable.
    expect(
      screen.queryByRole("button", { name: /安装 Codex/ }),
    ).not.toBeInTheDocument();
    expect(sessionStorage.getItem("cam.win.provenance-recovery")).toContain(
      "win-op-1",
    );

    await act(async () => {
      statusProbe.resolve({ installed: null, status: "none" });
      await statusProbe.promise;
    });
    expect(
      await screen.findByRole("button", { name: "重新检查" }),
    ).toBeInTheDocument();
  });

  it("does not let an old perform finally release a newer recovery generation", async () => {
    const oldCompletion = deferred<OperationCompletion | null>();
    const newCompletion = deferred<OperationCompletion | null>();
    const oldStatusProbe = deferred<WinInstallStatus>();
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.winStatus
      .mockResolvedValueOnce({ installed: null, status: "none" })
      .mockReturnValueOnce(oldStatusProbe.promise)
      .mockResolvedValue({ installed: null, status: "none" });
    api.winPlanUpdate.mockResolvedValue(report({ installed: null }));
    api.winPerformUpdate.mockRejectedValue({
      code: "network_error",
      message: "late failure",
    });
    api.getOperationCompletion.mockImplementation((token) =>
      token === "win-op-1" ? oldCompletion.promise : newCompletion.promise,
    );

    const user = userEvent.setup();
    renderWinHome();
    await user.click(await screen.findByRole("button", { name: /安装 Codex/ }));
    await waitFor(() =>
      expect(api.getOperationCompletion).toHaveBeenCalledWith("win-op-1"),
    );

    sessionStorage.setItem(
      "cam.win.provenance-recovery",
      JSON.stringify({ state: "unknown", token: "new-op" }),
    );
    await act(async () => {
      oldCompletion.resolve({
        id: "win-op-1",
        kind: "update",
        phase: "downloading",
        state: "failed-before-commit",
      });
      await oldCompletion.promise;
    });
    await waitFor(() =>
      expect(api.getOperationCompletion).toHaveBeenCalledWith("new-op"),
    );

    await act(async () => {
      oldStatusProbe.resolve({ installed: null, status: "none" });
      await oldStatusProbe.promise;
    });
    // The old run has now unwound through its finally. Its generation no longer
    // owns busy/resetStop, so the newer reconciliation remains visibly active.
    expect(
      screen.getByText("正在检查…", { selector: ".headline" }),
    ).toBeInTheDocument();
    expect(sessionStorage.getItem("cam.win.provenance-recovery")).toContain(
      "new-op",
    );

    await act(async () => {
      newCompletion.resolve(null);
      await newCompletion.promise;
    });
    expect(
      await screen.findByRole("button", { name: "重新检查" }),
    ).toBeInTheDocument();
  });

  it("treats a stale expectation as a notice and re-checks", async () => {
    const user = userEvent.setup();
    api.getSettings.mockResolvedValue(settings({ askBefore: false }));
    api.winPerformUpdate.mockRejectedValue({
      code: "stale_expectation",
      message: "stale",
    });
    api.winPlanUpdate.mockResolvedValueOnce(report()).mockResolvedValue(
      report({
        plan: { ...PLAN_UPDATE, upToDate: true, latestVersion: "1.0.0" },
      }),
    );
    renderWinHome();
    await user.click(await screen.findByRole("button", { name: /立即更新/ }));
    expect(await screen.findByText(/安装状态已变化/)).toBeInTheDocument();
    expect(
      await screen.findByText("已是最新", { selector: ".headline" }),
    ).toBeInTheDocument();
    expect(sessionStorage.getItem("cam.win.provenance-recovery")).toBeNull();
  });

  it("warns when MSIX is planned but App Installer is missing, and flips to portable", async () => {
    const user = userEvent.setup();
    api.winPlanUpdate.mockResolvedValue(
      report({ capabilities: { ...CAPS_OK, appInstaller: unknown } }),
    );
    api.setSettings.mockImplementation((next: AppSettings) =>
      Promise.resolve(next),
    );
    renderWinHome();

    const switchBtn = await screen.findByRole("button", {
      name: /改用免安装版/,
    });
    await user.click(switchBtn);
    await waitFor(() =>
      expect(api.setSettings).toHaveBeenCalledWith(
        expect.objectContaining({ windowsInstallMode: "portable" }),
      ),
    );
    // Switching re-plans so the route (and the banner) can settle.
    await waitFor(() =>
      expect(api.winPlanUpdate.mock.calls.length).toBeGreaterThanOrEqual(2),
    );
  });

  it("closes the install-dir sheet when a focus re-check finds the install drifted", async () => {
    const user = userEvent.setup();
    // Capture the focus-recheck listener so the test can fire it.
    let onFocus: (() => void) | undefined;
    vi.mocked(listen).mockImplementation((event: string, cb: unknown) => {
      if (event === "tauri://focus") onFocus = cb as () => void;
      return Promise.resolve(() => {});
    });
    // Portable fresh-install: no install yet, so clicking install opens the
    // install-dir sheet instead of running straight away.
    api.getSettings.mockResolvedValue(
      settings({ windowsInstallMode: "portable" }),
    );
    api.winStatus.mockResolvedValue({ installed: null, status: "none" });
    api.winPlanUpdate.mockResolvedValue(
      report({
        installed: null,
        plan: { ...PLAN_UPDATE, route: "portable-fallback" },
      }),
    );
    renderWinHome();

    await user.click(await screen.findByRole("button", { name: /安装 Codex/ }));
    // The location sheet is up (fresh portable install needs a target).
    expect(await screen.findByText("选择安装位置")).toBeInTheDocument();

    // Codex appears out-of-band; the focus probe now sees a managed install —
    // an identity change from the null snapshot the sheet was built against.
    api.winStatus.mockResolvedValue(STATUS_MANAGED);
    await waitFor(() => expect(onFocus).toBeDefined());
    await act(async () => {
      onFocus?.();
      await Promise.resolve();
    });

    // The stale install-dir sheet must be gone — otherwise its 使用当前位置 /
    // 浏览 buttons could still run install against the vanished snapshot,
    // bypassing the external→adopt boundary.
    await waitFor(() =>
      expect(screen.queryByText("选择安装位置")).not.toBeInTheDocument(),
    );
  });

  it("closes the version picker when a focus re-check finds the install drifted", async () => {
    const user = userEvent.setup();
    let onFocus: (() => void) | undefined;
    vi.mocked(listen).mockImplementation((event: string, cb: unknown) => {
      if (event === "tauri://focus") onFocus = cb as () => void;
      return Promise.resolve(() => {});
    });
    renderWinHome();

    await user.click(
      await screen.findByRole("button", { name: /选择安装版本/ }),
    );
    expect(
      await screen.findByRole("dialog", { name: "选择安装版本" }),
    ).toBeInTheDocument();

    api.winStatus.mockResolvedValue({
      installed: INSTALLED,
      status: "external",
    });
    await waitFor(() => expect(onFocus).toBeDefined());
    await act(async () => {
      onFocus?.();
      await Promise.resolve();
    });

    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: "选择安装版本" }),
      ).not.toBeInTheDocument(),
    );
  });

  it.each([
    { intent: "pause" as const, outcome: "false" as const },
    { intent: "pause" as const, outcome: "reject" as const },
    { intent: "cancel" as const, outcome: "false" as const },
    { intent: "cancel" as const, outcome: "reject" as const },
  ])(
    "keeps the Windows progress flow recoverable when $intent returns $outcome",
    async ({ intent, outcome }) => {
      const user = userEvent.setup();
      api.getSettings.mockResolvedValue(settings({ askBefore: false }));
      api.winPerformUpdate.mockImplementationOnce(
        () => new Promise<WinPerformReport>(() => {}),
      );

      let onProgress:
        ((event: { payload: DownloadProgress }) => void) | undefined;
      listenMock.mockImplementation((event: string, cb: unknown) => {
        if (event === "win://download-progress")
          onProgress = cb as typeof onProgress;
        return Promise.resolve(() => {});
      });

      const stop =
        intent === "pause" ? api.winPauseDownload : api.winCancelDownload;
      if (outcome === "false") {
        stop.mockResolvedValue(false);
      } else {
        stop.mockRejectedValue(new Error("invoke bridge unavailable"));
      }

      renderWinHome();
      await user.click(await screen.findByRole("button", { name: /立即更新/ }));
      if (intent === "pause") {
        await waitFor(() => expect(onProgress).toBeDefined());
        act(() =>
          onProgress?.({
            payload: { downloaded: 10, total: 100, source: "mirror.example" },
          }),
        );
      }

      const action = intent === "pause" ? "暂停" : "取消";
      const button = await screen.findByRole("button", { name: action });
      await waitFor(() => expect(button).toBeEnabled());
      api.getOperationSnapshot.mockResolvedValue(ACTIVE_OPERATION);
      await user.click(button);

      const expected =
        outcome === "false"
          ? `${action}请求被后端拒绝。任务仍在继续，可重试。`
          : `${action}请求未送达。任务仍在继续，可重试。`;
      expect(await screen.findByRole("alert")).toHaveTextContent(expected);
      expect(screen.getByText("正在更新…")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: action })).toBeEnabled();

      await user.click(screen.getByRole("button", { name: action }));
      await waitFor(() => expect(stop).toHaveBeenCalledTimes(2));
    },
  );

  it("keeps the Windows paused screen and both recovery actions when discard rejects", async () => {
    const user = userEvent.setup();
    api.getSettings.mockResolvedValue(settings({ askBefore: false }));
    api.winDiscardDownload.mockRejectedValueOnce(new Error("cache locked"));

    let rejectPerform: ((cause: unknown) => void) | undefined;
    let onProgress:
      ((event: { payload: DownloadProgress }) => void) | undefined;
    listenMock.mockImplementation((event: string, cb: unknown) => {
      if (event === "win://download-progress")
        onProgress = cb as typeof onProgress;
      return Promise.resolve(() => {});
    });
    api.winPerformUpdate.mockImplementationOnce(
      () =>
        new Promise<WinPerformReport>(
          (_resolve, reject) => (rejectPerform = reject),
        ),
    );

    renderWinHome();
    await user.click(await screen.findByRole("button", { name: /立即更新/ }));
    await waitFor(() => expect(onProgress).toBeDefined());
    act(() =>
      onProgress?.({
        payload: {
          downloaded: 10,
          total: 100,
          source: "s",
          operationId: "op-active",
        },
      }),
    );
    await user.click(await screen.findByRole("button", { name: /^暂停$/ }));
    act(() => rejectPerform?.(new Error("download cancelled")));
    await screen.findByText("下载已暂停");

    await user.click(screen.getByRole("button", { name: "取消" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "取消未完成。下载仍处于暂停状态；你可以继续下载或重试取消。",
    );
    expect(screen.getByText("下载已暂停")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "继续" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "取消" })).toBeEnabled();

    await user.click(screen.getByRole("button", { name: "取消" }));
    await waitFor(() =>
      expect(api.winDiscardDownload).toHaveBeenCalledTimes(2),
    );
    expect(await screen.findByText("下载已取消。")).toBeInTheDocument();
  });
});
