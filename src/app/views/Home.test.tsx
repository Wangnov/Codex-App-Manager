import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { listen } from "@tauri-apps/api/event";

import { managerApi } from "../../services/managerApi";
import type {
  AppSettings,
  DownloadProgress,
  InstalledCodex,
  MacInstallStatus,
  MacPerformReport,
  MacUpdateReport,
  OperationSnapshot,
  UpdatePlan,
} from "../../shared/types";
import { DEFAULT_SETTINGS, emptyOperationOutcome } from "../../shared/types";
import { I18nProvider } from "../i18n";
import { ThemeProvider } from "../theme";
import { Home } from "./Home";

// The state machine is what's under test — the GSAP choreography isn't, and
// SplitText/DrawSVG don't run reliably under jsdom.
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
      getHostArchitecture: vi.fn(),
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
      macStatus: vi.fn(),
      macPlanUpdate: vi.fn(),
      macPerformUpdate: vi.fn(),
      macInstallHistoricalRelease: vi.fn(),
      macInstall: vi.fn(),
      macAdopt: vi.fn(),
      macAdoptPath: vi.fn(),
      macLaunch: vi.fn(),
      macPauseDownload: vi.fn(),
      macCancelDownload: vi.fn(),
      macDiscardDownload: vi.fn(),
      macPickExistingInstall: vi.fn(),
    },
  };
});

const api = vi.mocked(managerApi);
const listenMock = vi.mocked(listen);

const INSTALLED: InstalledCodex = {
  path: "/Applications/Codex.app",
  build: 100,
  shortVersion: "1.0.0",
  arch: "arm64",
};

const PLAN_UPDATE: UpdatePlan = {
  upToDate: false,
  currentBuild: 100,
  latestBuild: 200,
  latestShortVersion: "2.0.0",
  strategy: { kind: "full" },
  downloadUrl: "https://example.invalid/codex.delta",
  downloadSize: 1024,
  edSignature: null,
  fullSize: 4096,
  savingsPct: 0,
};

const REPORT_UPDATE: MacUpdateReport = {
  appcastUrl: "https://example.invalid/appcast.xml",
  installed: INSTALLED,
  simulatedBuild: null,
  plan: PLAN_UPDATE,
};

const REPORT_UPTODATE: MacUpdateReport = {
  ...REPORT_UPDATE,
  plan: {
    ...PLAN_UPDATE,
    upToDate: true,
    latestBuild: 100,
    latestShortVersion: "1.0.0",
  },
};

const STATUS_MANAGED: MacInstallStatus = {
  installed: INSTALLED,
  status: "managed",
};
const STATUS_NONE: MacInstallStatus = { installed: null, status: "none" };
const ACTIVE_OPERATION: OperationSnapshot = {
  id: "op-active",
  kind: "update",
  phase: "downloading",
  progress: { downloaded: 10, total: 100, source: "mirror.example" },
  paused: false,
  cancellable: true,
  interruptible: true,
};

const PERFORM_OK: MacPerformReport = {
  upToDate: false,
  fromBuild: 100,
  toBuild: 200,
  strategy: "full",
  installedPath: INSTALLED.path,
  verified: true,
  relaunched: true,
  relaunchFailed: false,
  rolledBack: false,
  warning: null,
  message: "ok",
};

function settings(overrides: Partial<AppSettings> = {}): AppSettings {
  return { ...DEFAULT_SETTINGS, ...overrides };
}

function setPlatform(platform: string) {
  Object.defineProperty(navigator, "platform", {
    configurable: true,
    value: platform,
  });
}

function renderHome() {
  return render(
    <ThemeProvider>
      <I18nProvider>
        <Home onOpenSettings={vi.fn()} />
      </I18nProvider>
    </ThemeProvider>,
  );
}

describe("MacHome state machine", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "zh-CN");
    setPlatform("MacIntel");
    api.getSettings.mockResolvedValue(settings());
    api.getSettingsStrict.mockResolvedValue(settings());
    api.macStatus.mockResolvedValue(STATUS_MANAGED);
    api.macPlanUpdate.mockResolvedValue(REPORT_UPDATE);
    api.macPerformUpdate.mockResolvedValue(PERFORM_OK);
    api.macPauseDownload.mockResolvedValue(true);
    api.macCancelDownload.mockResolvedValue(true);
    api.macDiscardDownload.mockResolvedValue(undefined);
    api.getOperationSnapshot.mockResolvedValue(null);
    api.getPausedOperationSnapshot.mockResolvedValue(null);
    api.getOperationCompletion.mockResolvedValue(null);
    api.beginTrackedOperation.mockResolvedValue("test-install-operation");
    api.armDestructive.mockResolvedValue("test-operation");
    api.checkManagerUpdate.mockResolvedValue({ kind: "none" });
    api.getHostArchitecture.mockResolvedValue("aarch64");
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
    api.macStatus.mockResolvedValue(STATUS_NONE);
    renderHome();
    expect(
      await screen.findByRole("button", { name: /安装 Codex/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /选择安装版本/ }),
    ).toBeInTheDocument();
    expect(screen.getByText("未检测到 Codex")).toBeInTheDocument();
  });

  it("classifies an available update and shows both versions in the meta list", async () => {
    renderHome();
    expect(
      await screen.findByText("有新版本", { selector: ".headline" }),
    ).toBeInTheDocument();
    // The meta list pairs the update target with the local install.
    expect(screen.getByText("2.0.0")).toBeInTheDocument();
    expect(screen.getByText("1.0.0")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /立即更新/ })).toBeEnabled();
  });

  it("routes the update CTA through the confirm sheet when askBefore is on", async () => {
    const user = userEvent.setup();
    renderHome();
    await user.click(await screen.findByRole("button", { name: /立即更新/ }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("更新到 2.0.0?");
    await user.click(screen.getByRole("button", { name: "更新" }));
    await waitFor(() =>
      expect(api.macPerformUpdate).toHaveBeenCalledWith({
        fromBuild: 100,
        toBuild: 200,
        path: INSTALLED.path,
        fromVersion: "1.0.0",
        toVersion: "2.0.0",
      }),
    );
  });

  it("routes a selected GitHub Release through the tracked historical installer", async () => {
    api.historicalReleaseCatalog.mockResolvedValue({
      repository: "Wangnov/codex-app-mirror",
      platform: "macos",
      architecture: "arm64",
      releases: [
        {
          tag: "codex-app-0.9.0",
          version: "0.9.0",
          publishedAt: "2026-01-01T00:00:00Z",
          assets: [
            {
              name: "Codex-mac-arm64.dmg",
              size: 1024,
              architecture: "arm64",
              format: "dmg",
              packageVersion: null,
            },
          ],
        },
      ],
    });
    api.macInstallHistoricalRelease.mockResolvedValue({
      installed: { ...INSTALLED, build: 90, shortVersion: "0.9.0" },
      status: "managed",
      outcome: emptyOperationOutcome({
        appState: "present",
        installClass: "managed",
        warnings: ["Codex 已安装，但自动重新打开失败；旧版本备份仍保留"],
      }),
    });
    const user = userEvent.setup();
    renderHome();

    await user.click(
      await screen.findByRole("button", { name: /选择安装版本/ }),
    );
    await user.click(await screen.findByRole("button", { name: /0\.9\.0/ }));
    await user.click(screen.getByRole("button", { name: /下载并安装/ }));

    await waitFor(() =>
      expect(api.macInstallHistoricalRelease).toHaveBeenCalledWith(
        expect.objectContaining({
          releaseTag: "codex-app-0.9.0",
          assetName: "Codex-mac-arm64.dmg",
          format: "dmg",
          localPath: null,
        }),
        true,
        { path: INSTALLED.path, build: INSTALLED.build },
        "test-operation",
      ),
    );
    expect(api.armDestructive).toHaveBeenCalledWith("update");
    expect(
      await screen.findByText(/自动重新打开失败；旧版本备份仍保留/),
    ).toBeInTheDocument();
  });

  it("uses an install operation token for a fresh historical install", async () => {
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.macStatus.mockResolvedValue(STATUS_NONE);
    api.historicalReleaseCatalog.mockResolvedValue({
      repository: "Wangnov/codex-app-mirror",
      platform: "macos",
      architecture: "arm64",
      releases: [
        {
          tag: "codex-app-0.9.0",
          version: "0.9.0",
          publishedAt: "2026-01-01T00:00:00Z",
          assets: [
            {
              name: "Codex-mac-arm64.dmg",
              size: 1024,
              architecture: "arm64",
              format: "dmg",
              packageVersion: null,
            },
          ],
        },
      ],
    });
    api.macInstallHistoricalRelease.mockResolvedValue({
      installed: { ...INSTALLED, build: 90, shortVersion: "0.9.0" },
      status: "managed",
    });
    const user = userEvent.setup();
    renderHome();

    await user.click(
      await screen.findByRole("button", { name: /选择安装版本/ }),
    );
    await user.click(screen.getByRole("button", { name: "arm64" }));
    await user.click(await screen.findByRole("button", { name: /0\.9\.0/ }));
    await user.click(screen.getByRole("button", { name: /下载并安装/ }));

    await waitFor(() =>
      expect(api.beginTrackedOperation).toHaveBeenCalledWith("install"),
    );
    expect(api.armDestructive).not.toHaveBeenCalledWith("install");
    expect(api.macInstallHistoricalRelease).toHaveBeenCalledWith(
      expect.objectContaining({ releaseTag: "codex-app-0.9.0" }),
      true,
      { path: null, build: null },
      "test-install-operation",
    );
  });

  it("keeps a paused historical install and its original expectation when lease acquisition fails", async () => {
    const confirmedExpectation = {
      platform: "macos" as const,
      currentPath: "/Applications/Confirmed Codex.app",
      currentBuild: 77,
    };
    const selection = {
      releaseTag: "codex-app-0.9.0",
      version: "0.9.0",
      assetName: "Codex-mac-arm64.dmg",
      architecture: "arm64" as const,
      format: "dmg" as const,
      packageVersion: null,
      localPath: null,
      localFileName: null,
    };
    api.getPausedOperationSnapshot.mockResolvedValue({
      id: "historical-mac-pause",
      kind: "update",
      phase: "downloading",
      progress: {
        downloaded: 512,
        total: 1024,
        source: "github.com",
        operationId: "historical-mac-pause",
      },
      paused: true,
      cancellable: true,
      interruptible: true,
      historical: {
        selection,
        blockUpdates: true,
        expectation: confirmedExpectation,
      },
    });
    // The live status has drifted since the user originally confirmed. Resume
    // must keep the frozen expectation so the backend can reject the drift.
    api.macStatus.mockResolvedValue(STATUS_MANAGED);
    let rejectLease: ((cause: unknown) => void) | undefined;
    api.armDestructive
      .mockImplementationOnce(
        () =>
          new Promise((_resolve, reject) => {
            rejectLease = reject;
          }),
      )
      .mockResolvedValueOnce("resumed-historical-mac");
    api.macInstallHistoricalRelease.mockResolvedValue({
      installed: { ...INSTALLED, build: 90, shortVersion: "0.9.0" },
      status: "managed",
    });

    const user = userEvent.setup();
    renderHome();
    expect(await screen.findByText("下载已暂停")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(rejectLease).toBeDefined());
    expect(
      screen.queryByRole("button", { name: "继续" }),
    ).not.toBeInTheDocument();
    expect(api.macDiscardDownload).not.toHaveBeenCalled();
    await act(async () => {
      rejectLease?.(new Error("another operation owns the lease"));
      await Promise.resolve();
    });
    expect(await screen.findByRole("alert")).toHaveTextContent("操作未完成");
    expect(screen.getByText("下载已暂停")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "继续" })).toBeEnabled();

    await user.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() =>
      expect(api.macInstallHistoricalRelease).toHaveBeenCalledWith(
        selection,
        true,
        {
          path: confirmedExpectation.currentPath,
          build: confirmedExpectation.currentBuild,
        },
        "resumed-historical-mac",
      ),
    );
  });

  it("performs immediately when askBefore is off", async () => {
    api.getSettings.mockResolvedValue(settings({ askBefore: false }));
    const user = userEvent.setup();
    renderHome();
    await user.click(await screen.findByRole("button", { name: /立即更新/ }));
    await waitFor(() => expect(api.macPerformUpdate).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("resumes an ordinary update with the original target after latest refreshes", async () => {
    api.getPausedOperationSnapshot.mockResolvedValue({
      id: "ordinary-mac-pause",
      kind: "update",
      phase: "downloading",
      progress: { downloaded: 256, total: 1024, source: "github.com" },
      paused: true,
      cancellable: true,
      interruptible: true,
      resume: {
        kind: "perform",
        installRoot: null,
        expectation: {
          platform: "macos",
          currentBuild: 100,
          targetBuild: 150,
          installPath: INSTALLED.path,
          currentVersion: "1.0.0",
          targetVersion: "1.5.0",
        },
      },
    });

    const user = userEvent.setup();
    renderHome();
    expect(await screen.findByText("下载已暂停")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "继续" }));

    await waitFor(() =>
      expect(api.macPerformUpdate).toHaveBeenCalledWith({
        fromBuild: 100,
        toBuild: 150,
        path: INSTALLED.path,
        fromVersion: "1.0.0",
        toVersion: "1.5.0",
      }),
    );
  });

  it("settles on up-to-date", async () => {
    api.macPlanUpdate.mockResolvedValue(REPORT_UPTODATE);
    renderHome();
    expect(
      await screen.findByText("已是最新", { selector: ".headline" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /启动 Codex/ })).toBeEnabled();
  });

  it("gates an external install behind adopt instead of offering the update", async () => {
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.macStatus.mockResolvedValue({
      installed: INSTALLED,
      status: "external",
    });
    const user = userEvent.setup();
    renderHome();
    const adopt = await screen.findByRole("button", { name: /开始管理/ });
    expect(
      screen.queryByRole("button", { name: /立即更新/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /选择安装版本/ }),
    ).not.toBeInTheDocument();
    api.macAdopt.mockResolvedValue(STATUS_MANAGED);
    await user.click(adopt);
    await waitFor(() => expect(api.macAdopt).toHaveBeenCalledTimes(1));
  });

  it("closes the version picker when a focus re-check finds the install drifted", async () => {
    const user = userEvent.setup();
    let onFocus: (() => void) | undefined;
    listenMock.mockImplementation((event: string, cb: unknown) => {
      if (event === "tauri://focus") onFocus = cb as () => void;
      return Promise.resolve(() => {});
    });
    renderHome();

    await user.click(
      await screen.findByRole("button", { name: /选择安装版本/ }),
    );
    expect(
      await screen.findByRole("dialog", { name: "选择安装版本" }),
    ).toBeInTheDocument();

    api.macStatus.mockResolvedValue({
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

  it("shows the error hero with a retry when the check fails and nothing is installed", async () => {
    api.macStatus.mockRejectedValue(new Error("unsupported"));
    api.macPlanUpdate.mockRejectedValue(new Error("appcast unreachable"));
    renderHome();
    expect(await screen.findByText("检查失败")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /重新检查/ })).toBeEnabled();
  });

  it("keeps local package installation reachable when only the online check fails", async () => {
    api.macStatus.mockResolvedValue(STATUS_NONE);
    api.macPlanUpdate.mockRejectedValue(new Error("appcast unreachable"));
    api.historicalReleaseCatalog.mockRejectedValue(
      new Error("github unreachable"),
    );
    const user = userEvent.setup();
    renderHome();

    expect(await screen.findByText("检查失败")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /选择安装版本/ }));
    await user.click(screen.getByRole("button", { name: "arm64" }));
    expect(
      await screen.findByRole("button", { name: /从本地安装包安装/ }),
    ).toBeEnabled();
  });

  it("treats a stale expectation as a notice and re-checks, not as an error", async () => {
    const user = userEvent.setup();
    api.getSettings.mockResolvedValue(settings({ askBefore: false }));
    api.macPerformUpdate.mockRejectedValue({
      code: "stale_expectation",
      message: "stale",
    });
    // First plan (startup check) offers the update; the stale-recovery
    // re-check finds reality moved on and settles up-to-date.
    api.macPlanUpdate
      .mockResolvedValueOnce(REPORT_UPDATE)
      .mockResolvedValue(REPORT_UPTODATE);
    renderHome();
    await user.click(await screen.findByRole("button", { name: /立即更新/ }));
    expect(await screen.findByText(/安装状态已变化/)).toBeInTheDocument();
    expect(screen.queryByText("stale")).not.toBeInTheDocument();
    expect(
      await screen.findByText("已是最新", { selector: ".headline" }),
    ).toBeInTheDocument();
  });

  it("pauses into a resumable screen and resumes the same operation", async () => {
    const user = userEvent.setup();
    api.getSettings.mockResolvedValue(settings({ askBefore: false }));

    // Capture the download-progress listener so the test can feed real bytes.
    let onProgress:
      ((event: { payload: DownloadProgress }) => void) | undefined;
    listenMock.mockImplementation((event: string, cb: unknown) => {
      if (event === "mac://download-progress") {
        onProgress = cb as typeof onProgress;
      }
      return Promise.resolve(() => {});
    });

    // First perform hangs until we reject it as a pause-cancel.
    let rejectPerform: ((cause: unknown) => void) | undefined;
    api.macPerformUpdate.mockImplementationOnce(
      () =>
        new Promise<MacPerformReport>((_resolve, reject) => {
          rejectPerform = reject;
        }),
    );

    renderHome();
    await user.click(await screen.findByRole("button", { name: /立即更新/ }));
    expect(await screen.findByText("正在更新…")).toBeInTheDocument();

    // Bytes arrive → the pause button becomes actionable.
    await waitFor(() => expect(onProgress).toBeDefined());
    act(() => {
      onProgress?.({
        payload: {
          downloaded: 512,
          total: 1024,
          source: "mirror.example",
          operationId: "op-active",
        },
      });
    });

    // The progress bar exposes progressbar semantics to assistive tech. The
    // exact aria-valuenow eases (useCountUp), so assert the range + presence
    // rather than a timing-dependent value.
    const progressbar = await screen.findByRole("progressbar");
    expect(progressbar).toHaveAttribute("aria-valuemin", "0");
    expect(progressbar).toHaveAttribute("aria-valuemax", "100");
    expect(progressbar).toHaveAttribute("aria-valuenow");

    const pause = await screen.findByRole("button", { name: /^暂停$/ });
    await waitFor(() => expect(pause).toBeEnabled());
    await user.click(pause);
    await waitFor(() => expect(api.macPauseDownload).toHaveBeenCalledTimes(1));
    expect(api.macPauseDownload).toHaveBeenCalledWith("op-active");

    // The backend acknowledges the pause by failing the in-flight perform.
    act(() => rejectPerform?.(new Error("download cancelled")));

    expect(await screen.findByText("下载已暂停")).toBeInTheDocument();
    const resume = screen.getByRole("button", { name: /继续/ });
    await user.click(resume);
    // Resume re-runs the SAME operation (perform, not install).
    await waitFor(() => expect(api.macPerformUpdate).toHaveBeenCalledTimes(2));
  });

  it("cancels a paused download only after the partial is discarded", async () => {
    const user = userEvent.setup();
    api.getSettings.mockResolvedValue(settings({ askBefore: false }));
    let rejectPerform: ((cause: unknown) => void) | undefined;
    let onProgress:
      ((event: { payload: DownloadProgress }) => void) | undefined;
    listenMock.mockImplementation((event: string, cb: unknown) => {
      if (event === "mac://download-progress")
        onProgress = cb as typeof onProgress;
      return Promise.resolve(() => {});
    });
    api.macPerformUpdate.mockImplementationOnce(
      () =>
        new Promise<MacPerformReport>((_r, reject) => (rejectPerform = reject)),
    );

    renderHome();
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

    await user.click(screen.getByRole("button", { name: /取消/ }));
    await waitFor(() =>
      expect(api.macDiscardDownload).toHaveBeenCalledTimes(1),
    );
    expect(await screen.findByText("下载已取消。")).toBeInTheDocument();
  });

  it.each([
    { intent: "pause" as const, outcome: "false" as const },
    { intent: "pause" as const, outcome: "reject" as const },
    { intent: "cancel" as const, outcome: "false" as const },
    { intent: "cancel" as const, outcome: "reject" as const },
  ])(
    "keeps the macOS progress flow recoverable when $intent returns $outcome",
    async ({ intent, outcome }) => {
      const user = userEvent.setup();
      api.getSettings.mockResolvedValue(settings({ askBefore: false }));
      api.macPerformUpdate.mockImplementationOnce(
        () => new Promise<MacPerformReport>(() => {}),
      );

      let onProgress:
        ((event: { payload: DownloadProgress }) => void) | undefined;
      listenMock.mockImplementation((event: string, cb: unknown) => {
        if (event === "mac://download-progress")
          onProgress = cb as typeof onProgress;
        return Promise.resolve(() => {});
      });

      const stop =
        intent === "pause" ? api.macPauseDownload : api.macCancelDownload;
      if (outcome === "false") {
        stop.mockResolvedValue(false);
      } else {
        stop.mockRejectedValue(new Error("invoke bridge unavailable"));
      }

      renderHome();
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

      // The same control becomes actionable again rather than staying pending.
      await user.click(screen.getByRole("button", { name: action }));
      await waitFor(() => expect(stop).toHaveBeenCalledTimes(2));
    },
  );

  it("keeps the macOS paused screen and both recovery actions when discard rejects", async () => {
    const user = userEvent.setup();
    api.getSettings.mockResolvedValue(settings({ askBefore: false }));
    api.macDiscardDownload.mockRejectedValueOnce(new Error("cache locked"));

    let rejectPerform: ((cause: unknown) => void) | undefined;
    let onProgress:
      ((event: { payload: DownloadProgress }) => void) | undefined;
    listenMock.mockImplementation((event: string, cb: unknown) => {
      if (event === "mac://download-progress")
        onProgress = cb as typeof onProgress;
      return Promise.resolve(() => {});
    });
    api.macPerformUpdate.mockImplementationOnce(
      () =>
        new Promise<MacPerformReport>(
          (_resolve, reject) => (rejectPerform = reject),
        ),
    );

    renderHome();
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

    // The failed discard is retryable; only the successful retry leaves pause.
    await user.click(screen.getByRole("button", { name: "取消" }));
    await waitFor(() =>
      expect(api.macDiscardDownload).toHaveBeenCalledTimes(2),
    );
    expect(await screen.findByText("下载已取消。")).toBeInTheDocument();
  });
});
