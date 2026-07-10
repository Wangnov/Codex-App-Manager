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
  UpdatePlan,
} from "../../shared/types";
import { DEFAULT_SETTINGS } from "../../shared/types";
import { I18nProvider } from "../i18n";
import { ThemeProvider } from "../theme";
import { Home } from "./Home";

// The state machine is what's under test — the GSAP choreography isn't, and
// SplitText/DrawSVG don't run reliably under jsdom.
vi.mock("../motion", () => ({ useHomeMotion: () => {} }));

vi.mock("../../services/managerApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../services/managerApi")>();
  return {
    ...actual,
    managerApi: {
      getSettings: vi.fn(),
      setSettings: vi.fn(),
      getOperationSnapshot: vi.fn(() => Promise.resolve(null)),
      macStatus: vi.fn(),
      macPlanUpdate: vi.fn(),
      macPerformUpdate: vi.fn(),
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
  plan: { ...PLAN_UPDATE, upToDate: true, latestBuild: 100, latestShortVersion: "1.0.0" },
};

const STATUS_MANAGED: MacInstallStatus = { installed: INSTALLED, status: "managed" };
const STATUS_NONE: MacInstallStatus = { installed: null, status: "none" };

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
  Object.defineProperty(navigator, "platform", { configurable: true, value: platform });
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
    api.macStatus.mockResolvedValue(STATUS_MANAGED);
    api.macPlanUpdate.mockResolvedValue(REPORT_UPDATE);
    api.macPerformUpdate.mockResolvedValue(PERFORM_OK);
    api.macPauseDownload.mockResolvedValue(true);
    api.macCancelDownload.mockResolvedValue(true);
    api.macDiscardDownload.mockResolvedValue(undefined);
  });

  it("offers install when nothing is detected", async () => {
    api.getSettings.mockResolvedValue(settings({ checkOnStartup: false }));
    api.macStatus.mockResolvedValue(STATUS_NONE);
    renderHome();
    expect(await screen.findByRole("button", { name: /安装 Codex/ })).toBeInTheDocument();
    expect(screen.getByText("未检测到 Codex")).toBeInTheDocument();
  });

  it("classifies an available update and shows both versions in the meta list", async () => {
    renderHome();
    expect(await screen.findByText("有新版本", { selector: ".headline" })).toBeInTheDocument();
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
      }),
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
    api.macStatus.mockResolvedValue({ installed: INSTALLED, status: "external" });
    const user = userEvent.setup();
    renderHome();
    const adopt = await screen.findByRole("button", { name: /开始管理/ });
    expect(screen.queryByRole("button", { name: /立即更新/ })).not.toBeInTheDocument();
    api.macAdopt.mockResolvedValue(STATUS_MANAGED);
    await user.click(adopt);
    await waitFor(() => expect(api.macAdopt).toHaveBeenCalledTimes(1));
  });

  it("shows the error hero with a retry when the check fails and nothing is installed", async () => {
    api.macStatus.mockRejectedValue(new Error("unsupported"));
    api.macPlanUpdate.mockRejectedValue(new Error("appcast unreachable"));
    renderHome();
    expect(await screen.findByText("检查失败")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /重新检查/ })).toBeEnabled();
  });

  it("treats a stale expectation as a notice and re-checks, not as an error", async () => {
    const user = userEvent.setup();
    api.getSettings.mockResolvedValue(settings({ askBefore: false }));
    api.macPerformUpdate.mockRejectedValue({ code: "stale_expectation", message: "stale" });
    // First plan (startup check) offers the update; the stale-recovery
    // re-check finds reality moved on and settles up-to-date.
    api.macPlanUpdate.mockResolvedValueOnce(REPORT_UPDATE).mockResolvedValue(REPORT_UPTODATE);
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
    let onProgress: ((event: { payload: DownloadProgress }) => void) | undefined;
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
      onProgress?.({ payload: { downloaded: 512, total: 1024, source: "mirror.example" } });
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
    let onProgress: ((event: { payload: DownloadProgress }) => void) | undefined;
    listenMock.mockImplementation((event: string, cb: unknown) => {
      if (event === "mac://download-progress") onProgress = cb as typeof onProgress;
      return Promise.resolve(() => {});
    });
    api.macPerformUpdate.mockImplementationOnce(
      () => new Promise<MacPerformReport>((_r, reject) => (rejectPerform = reject)),
    );

    renderHome();
    await user.click(await screen.findByRole("button", { name: /立即更新/ }));
    await waitFor(() => expect(onProgress).toBeDefined());
    act(() => onProgress?.({ payload: { downloaded: 10, total: 100, source: "s" } }));
    await user.click(await screen.findByRole("button", { name: /^暂停$/ }));
    act(() => rejectPerform?.(new Error("download cancelled")));
    await screen.findByText("下载已暂停");

    await user.click(screen.getByRole("button", { name: /取消/ }));
    await waitFor(() => expect(api.macDiscardDownload).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("下载已取消。")).toBeInTheDocument();
  });
});
