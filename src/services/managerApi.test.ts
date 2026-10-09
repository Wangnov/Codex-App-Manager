import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { DEFAULT_SETTINGS } from "../shared/types";
import {
  isNetworkError,
  managerApi,
  SETTINGS_CHANGED_EVENT,
} from "./managerApi";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);

beforeEach(() => {
  invokeMock.mockReset();
  listenMock.mockReset();
  vi.stubGlobal("window", { open: vi.fn(), __TAURI_INTERNALS__: undefined });
  localStorage.clear();
});

describe("isNetworkError", () => {
  it("classifies transport and TLS failures as connectivity errors", () => {
    expect(
      isNetworkError(
        "update engine error: io error: curl failed for host=codexapp.agentsmirror.com exit=35: stderr='curl: (35) schannel: failed to receive handshake, SSL/TLS connection failed'",
      ),
    ).toBe(true);
    expect(
      isNetworkError(
        "curl: (6) Could not resolve host: codexapp.agentsmirror.com",
      ),
    ).toBe(true);
    expect(
      isNetworkError("curl: (28) Operation timed out after 20000 milliseconds"),
    ).toBe(true);
  });

  it("classifies the macOS auto-source fallback failure as connectivity", () => {
    expect(
      isNetworkError(
        "both the mirror and OpenAI official appcast are unreachable",
      ),
    ).toBe(true);
  });

  it("does not treat server responses or verification failures as connectivity", () => {
    expect(
      isNetworkError(
        "update engine error: curl failed for https://example.test/appcast.xml: curl: (22) The requested URL returned error: 404",
      ),
    ).toBe(false);
    expect(isNetworkError("appcast enclosure missing edSignature")).toBe(false);
    expect(isNetworkError("EdDSA signature does not match")).toBe(false);
  });
});

describe("settings API", () => {
  it("migrates legacy browser settings into startup and periodic checks", async () => {
    localStorage.setItem(
      "cam.settings",
      JSON.stringify({
        source: "mirror",
        customUrl: "",
        autoCheck: false,
        askBefore: true,
        signedOnly: true,
      }),
    );

    const settings = await managerApi.getSettings();

    expect(settings.source).toBe("mirror");
    expect(settings.autoCheck).toBe(false);
    expect(settings.checkOnStartup).toBe(false);
    expect(settings.periodicCheck).toBe(false);
    expect(settings.periodicCheckIntervalSeconds).toBe(15 * 60);
    expect(settings.disableCodexSelfUpdates).toBe(false);
  });

  it("normalizes and broadcasts browser settings writes", async () => {
    const dispatchEvent = vi.fn();
    vi.stubGlobal("window", {
      open: vi.fn(),
      __TAURI_INTERNALS__: undefined,
      dispatchEvent,
    });

    const saved = await managerApi.setSettings({
      ...DEFAULT_SETTINGS,
      periodicCheckIntervalSeconds: 0,
      disableCodexSelfUpdates: true,
    });

    expect(saved.periodicCheckIntervalSeconds).toBe(60);
    expect(saved.disableCodexSelfUpdates).toBe(true);
    expect(dispatchEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        type: SETTINGS_CHANGED_EVENT,
        detail: saved,
      }),
    );
  });

  it("lets automatic network callers fail closed on native settings errors", async () => {
    vi.stubGlobal("window", {
      open: vi.fn(),
      __TAURI_INTERNALS__: {},
    });
    invokeMock.mockRejectedValue(new Error("settings unavailable"));

    await expect(managerApi.getSettingsStrict()).rejects.toThrow(
      "settings unavailable",
    );
    expect(invokeMock).toHaveBeenCalledWith("get_settings_strict");
  });

  it("coerces empty custom source and proxy modes to real defaults", async () => {
    const dispatchEvent = vi.fn();
    vi.stubGlobal("window", {
      open: vi.fn(),
      __TAURI_INTERNALS__: undefined,
      dispatchEvent,
    });

    const saved = await managerApi.setSettings({
      ...DEFAULT_SETTINGS,
      source: "custom",
      customUrl: "  ",
      proxyMode: "custom",
      customProxyUrl: "",
    });

    expect(saved.source).toBe("auto");
    expect(saved.customUrl).toBe("");
    expect(saved.proxyMode).toBe("system");
    expect(saved.customProxyUrl).toBe("");
  });
});

describe("diagnostics API", () => {
  it("returns browser fallbacks without invoking Tauri", async () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    const diagnostics = await managerApi.getDiagnostics();

    expect(diagnostics.os).toBe("browser");
    await expect(managerApi.getHostArchitecture()).resolves.toBe("x86_64");
    await expect(managerApi.openLogsDir()).resolves.toBeUndefined();
    await expect(managerApi.openCodexHome()).resolves.toBeUndefined();
    await expect(
      managerApi.frontendReady("en", 1, "browser-token"),
    ).resolves.toBeUndefined();
    await expect(
      managerApi.reportFrontendError({
        kind: "test",
        message: "boom",
        stack: null,
        componentStack: null,
      }),
    ).resolves.toBeUndefined();
    expect(invokeMock).not.toHaveBeenCalled();
    expect(consoleError).toHaveBeenCalledWith(
      "[frontend]",
      expect.objectContaining({ kind: "test", message: "boom" }),
    );
    consoleError.mockRestore();
  });

  it("reports frontend readiness and application language through IPC", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValue(undefined);

    await expect(
      managerApi.frontendReady("zh-TW", 7, "generation-token"),
    ).resolves.toBeUndefined();

    expect(invokeMock).toHaveBeenCalledWith("frontend_ready", {
      lang: "zh-TW",
      generation: 7,
      token: "generation-token",
    });
  });

  it("copies text through native IPC inside Tauri", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValue(undefined);
    await managerApi.writeClipboardText("诊断信息\nCodex");
    expect(invokeMock).toHaveBeenCalledWith("write_clipboard_text", { text: "诊断信息\nCodex" });
  });

  it("invokes diagnostics commands inside Tauri", async () => {
    window.__TAURI_INTERNALS__ = {};
    const diagnostics = {
      appVersion: "0.1.17",
      os: "macos",
      arch: "aarch64",
      locale: null,
      updateSource: "auto",
      customSourceHost: null,
      windowsInstallMode: null,
      installStatus: "macos status=none",
      configHealth: {
        settingsStatus: "ok",
        provenanceStatus: "ok",
        unknownSource: null,
        detail: null,
        settingsBackupAvailable: false,
        provenanceBackupAvailable: false,
      },
      logsDir: "/tmp/logs",
      recentErrors: [],
      logTail: "",
      generatedAtUnix: 1,
    };
    invokeMock
      .mockResolvedValueOnce(diagnostics)
      .mockResolvedValueOnce("aarch64")
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(undefined);

    await expect(managerApi.getDiagnostics()).resolves.toEqual(diagnostics);
    await expect(managerApi.getHostArchitecture()).resolves.toBe("aarch64");
    await expect(managerApi.openLogsDir()).resolves.toBeUndefined();
    await expect(managerApi.openCodexHome()).resolves.toBeUndefined();
    await expect(
      managerApi.reportFrontendError({
        kind: "test",
        message: "boom",
        stack: null,
        componentStack: null,
      }),
    ).resolves.toBeUndefined();
    expect(invokeMock).toHaveBeenNthCalledWith(1, "get_diagnostics");
    expect(invokeMock).toHaveBeenNthCalledWith(2, "get_host_architecture");
    expect(invokeMock).toHaveBeenNthCalledWith(3, "open_logs_dir");
    expect(invokeMock).toHaveBeenNthCalledWith(4, "open_codex_home");
    expect(invokeMock).toHaveBeenNthCalledWith(5, "log_frontend_error", {
      payload: {
        kind: "test",
        message: "boom",
        stack: null,
        componentStack: null,
      },
    });
  });
});

describe("Windows perform API", () => {
  it("sends the renderer resume mode and one-shot install root to the backend", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValue({ success: true });

    await managerApi.winPerformUpdate(
      true,
      {
        currentVersion: null,
        latestVersion: "2.0.0",
        packageMoniker: "Codex_2.0.0_x64",
        route: "msix-sideload",
      },
      "D:\\Selected\\Codex",
      "resume-token",
      "install",
    );

    expect(invokeMock).toHaveBeenCalledWith("win_perform_update", {
      confirm: true,
      token: "resume-token",
      installRoot: "D:\\Selected\\Codex",
      expected: {
        currentVersion: null,
        latestVersion: "2.0.0",
        packageMoniker: "Codex_2.0.0_x64",
        route: "msix-sideload",
      },
      resumeKind: "install",
    });
  });
});

describe("macOS resumable target API", () => {
  it("sends exact update and fresh-install targets to the backend", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValue({});

    await managerApi.macPerformUpdate({
      fromBuild: 100,
      toBuild: 150,
      path: "/Applications/Codex.app",
      fromVersion: "1.0.0",
      toVersion: "1.5.0",
    });
    await managerApi.macInstall({
      targetBuild: 150,
      targetVersion: "1.5.0",
    });

    expect(invokeMock).toHaveBeenNthCalledWith(1, "arm_destructive", {
      kind: "update",
    });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "mac_perform_update", {
      confirm: true,
      token: {},
      expected: {
        fromBuild: 100,
        toBuild: 150,
        path: "/Applications/Codex.app",
        fromVersion: "1.0.0",
        toVersion: "1.5.0",
      },
    });
    expect(invokeMock).toHaveBeenNthCalledWith(3, "mac_install", {
      expectedTargetBuild: 150,
      expectedTargetVersion: "1.5.0",
    });
  });
});

describe("manager self-update API", () => {
  it("reports development when there is no Tauri runtime, without contacting the backend", async () => {
    const result = await managerApi.checkManagerUpdate();
    expect(result).toEqual({ kind: "development" });
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("treats a feed failure as unavailable rather than throwing", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockRejectedValueOnce(new Error("offline"));

    const result = await managerApi.checkManagerUpdate();

    expect(result).toEqual({ kind: "unavailable" });
  });

  it("reports none when the backend has no update", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValueOnce(null);

    const result = await managerApi.checkManagerUpdate();

    expect(result).toEqual({ kind: "none" });
    expect(invokeMock).toHaveBeenCalledWith("manager_check_update");
  });

  it("relaunches through the Rust backend (not the renderer process plugin) after installing", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValueOnce({
      version: "0.6.0",
      currentVersion: "0.5.10",
      body: "notes",
    });

    const result = await managerApi.checkManagerUpdate();
    expect(result.kind).toBe("available");
    if (result.kind !== "available") throw new Error("expected available");

    invokeMock.mockReset();
    invokeMock.mockResolvedValueOnce(undefined).mockResolvedValueOnce(undefined);

    await result.installAndRelaunch();

    expect(invokeMock).toHaveBeenNthCalledWith(1, "manager_install_update", {
      expectedVersion: "0.6.0",
      expectedCurrentVersion: "0.5.10",
    });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "manager_relaunch");
  });

  it("does not relaunch when the install itself fails", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValueOnce({
      version: "0.6.0",
      currentVersion: "0.5.10",
    });
    const result = await managerApi.checkManagerUpdate();
    if (result.kind !== "available") throw new Error("expected available");

    invokeMock.mockReset();
    invokeMock.mockRejectedValueOnce({
      code: "stale_expectation",
      message: "release changed",
    });

    await expect(result.installAndRelaunch()).rejects.toBeTruthy();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).not.toHaveBeenCalledWith("manager_relaunch");
  });
});

describe("manager self-update runtime snapshot", () => {
  it("reports the idle snapshot in development without contacting the backend", async () => {
    const snapshot = await managerApi.getManagerUpdateRuntime();
    expect(snapshot.phase).toBe("idle");
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("fetches the current snapshot from the backend", async () => {
    window.__TAURI_INTERNALS__ = {};
    const snapshot = {
      phase: "downloading" as const,
      version: "0.6.0",
      downloaded: 512,
      total: 2048,
      code: null,
      updatedAtMs: 1234,
    };
    invokeMock.mockResolvedValueOnce(snapshot);

    const result = await managerApi.getManagerUpdateRuntime();

    expect(result).toEqual(snapshot);
    expect(invokeMock).toHaveBeenCalledWith("manager_get_update_runtime");
  });

  it("acks a terminal snapshot through the backend", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValueOnce({
      phase: "idle",
      version: null,
      downloaded: 0,
      total: null,
      code: null,
      updatedAtMs: 0,
    });

    await managerApi.ackManagerUpdateRuntime();

    expect(invokeMock).toHaveBeenCalledWith("manager_ack_update_runtime");
  });

  it("relaunches standalone, without a Tauri runtime it is a silent no-op", async () => {
    await expect(managerApi.relaunchManager()).resolves.toBeUndefined();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("relaunches standalone through the backend so a view can finish a reattached install", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValueOnce(undefined);

    await managerApi.relaunchManager();

    expect(invokeMock).toHaveBeenCalledWith("manager_relaunch");
  });

  it("propagates a blocked relaunch (an uninterruptible operation elsewhere) to the caller", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockRejectedValueOnce(
      new Error("cannot relaunch while another operation is in progress"),
    );

    await expect(managerApi.relaunchManager()).rejects.toThrow(
      "cannot relaunch while another operation is in progress",
    );
  });

  it("subscribes to live snapshot updates and forwards each payload", async () => {
    window.__TAURI_INTERNALS__ = {};
    const unlisten = vi.fn();
    let handler: ((event: { payload: unknown }) => void) | undefined;
    listenMock.mockImplementation(async (_event, cb) => {
      handler = cb as typeof handler;
      return unlisten;
    });

    const onSnapshot = vi.fn();
    const dispose = await managerApi.onManagerUpdateRuntime(onSnapshot);

    expect(listenMock).toHaveBeenCalledWith(
      "manager://update-state",
      expect.any(Function),
    );
    const payload = {
      phase: "installing",
      version: "0.6.0",
      downloaded: 2048,
      total: 2048,
      code: null,
      updatedAtMs: 5678,
    };
    handler?.({ payload });
    expect(onSnapshot).toHaveBeenCalledWith(payload);

    dispose();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("resolves to a no-op unlisten function in development", async () => {
    const dispose = await managerApi.onManagerUpdateRuntime(vi.fn());
    expect(listenMock).not.toHaveBeenCalled();
    expect(() => dispose()).not.toThrow();
  });
});
