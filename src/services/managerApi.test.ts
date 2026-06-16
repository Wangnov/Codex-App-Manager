import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { isNetworkError, managerApi } from "./managerApi";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  vi.stubGlobal("window", { open: vi.fn(), __TAURI_INTERNALS__: undefined });
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
