import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { DEFAULT_SETTINGS } from "../shared/types";
import { isNetworkError, managerApi, SETTINGS_CHANGED_EVENT } from "./managerApi";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
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
    expect(isNetworkError("curl: (6) Could not resolve host: codexapp.agentsmirror.com")).toBe(
      true,
    );
    expect(isNetworkError("curl: (28) Operation timed out after 20000 milliseconds")).toBe(true);
  });

  it("classifies the macOS auto-source fallback failure as connectivity", () => {
    expect(
      isNetworkError("both the mirror and OpenAI official appcast are unreachable"),
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

describe("manager updater API", () => {
  it("keeps check and confirmed install as separate Tauri commands", async () => {
    window.__TAURI_INTERNALS__ = {};
    const metadata = {
      version: "2.0.0",
      currentVersion: "1.0.0",
      body: "release notes",
    };
    invokeMock.mockResolvedValueOnce(metadata).mockResolvedValueOnce(undefined);

    await expect(managerApi.checkManagerUpdate()).resolves.toEqual({
      kind: "available",
      ...metadata,
    });
    expect(invokeMock).toHaveBeenNthCalledWith(1, "manager_check_update");

    await expect(managerApi.installManagerUpdate(metadata)).resolves.toBeUndefined();
    expect(invokeMock).toHaveBeenNthCalledWith(2, "manager_install_update", {
      expectedVersion: "2.0.0",
      expectedCurrentVersion: "1.0.0",
      expectedBody: "release notes",
    });
  });

  it("exposes renderer-independent manager update runtime state", async () => {
    window.__TAURI_INTERNALS__ = {};
    const snapshot = {
      revision: 4,
      version: "2.0.0",
      currentVersion: "1.0.0",
      body: "release notes",
      phase: "downloading",
      downloaded: 5,
      total: 10,
      failure: null,
    } as const;
    invokeMock.mockResolvedValueOnce(snapshot);

    await expect(managerApi.getManagerUpdateRuntime()).resolves.toEqual(snapshot);
    expect(invokeMock).toHaveBeenCalledWith("manager_get_update_runtime");
  });

  it("acknowledges terminal runtime state with revision and target CAS fields", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValueOnce(true);

    await expect(
      managerApi.acknowledgeManagerUpdateRuntime({
        revision: 7,
        version: "2.0.0",
        currentVersion: "1.0.0",
      }),
    ).resolves.toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("manager_ack_update_runtime", {
      revision: 7,
      version: "2.0.0",
      currentVersion: "1.0.0",
    });
  });

  it("routes manager relaunch through the shared backend operation lock", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValueOnce(undefined);

    await expect(managerApi.relaunchManager()).resolves.toBeUndefined();
    expect(invokeMock).toHaveBeenCalledWith("manager_relaunch");
  });

  it("preserves updater command failures for structured recovery UI", async () => {
    window.__TAURI_INTERNALS__ = {};
    const failure = { code: "network", message: "offline" };
    invokeMock.mockRejectedValueOnce(failure);

    await expect(managerApi.checkManagerUpdate()).rejects.toEqual(failure);
  });
});

describe("diagnostics API", () => {
  it("returns browser fallbacks without invoking Tauri", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const diagnostics = await managerApi.getDiagnostics();

    expect(diagnostics.os).toBe("browser");
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
    expect(invokeMock).not.toHaveBeenCalled();
    expect(consoleError).toHaveBeenCalledWith(
      "[frontend]",
      expect.objectContaining({ kind: "test", message: "boom" }),
    );
    consoleError.mockRestore();
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
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(undefined);

    await expect(managerApi.getDiagnostics()).resolves.toEqual(diagnostics);
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
    expect(invokeMock).toHaveBeenNthCalledWith(2, "open_logs_dir");
    expect(invokeMock).toHaveBeenNthCalledWith(3, "open_codex_home");
    expect(invokeMock).toHaveBeenNthCalledWith(4, "log_frontend_error", {
      payload: {
        kind: "test",
        message: "boom",
        stack: null,
        componentStack: null,
      },
    });
  });
});
