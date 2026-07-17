import { describe, expect, it } from "vitest";

import capabilityJson from "../../src-tauri/capabilities/default.json";
import tauriConfigJson from "../../src-tauri/tauri.conf.json";

interface TauriConfig {
  app: {
    windows: Array<{ label: string; create?: boolean }>;
    security: { csp: string; devCsp: string };
  };
}

interface Capability {
  permissions: string[];
}

describe("desktop trust-boundary config", () => {
  it("keeps the main webview under the native navigation builder", () => {
    const config = tauriConfigJson as TauriConfig;
    expect(config.app.windows.find((window) => window.label === "main")?.create).toBe(false);
  });

  it("uses a local-only production CSP and a loopback-only HMR policy", () => {
    const { app } = tauriConfigJson as TauriConfig;
    const production = app.security.csp;
    expect(production).toContain("script-src 'self'");
    expect(production).toContain("connect-src 'self' ipc: http://ipc.localhost");
    expect(production).toContain("base-uri 'none'");
    expect(production).toContain("object-src 'none'");
    expect(production).toContain("form-action 'none'");
    expect(production).toContain("frame-ancestors 'none'");
    expect(production).not.toMatch(/github\.com|agentsmirror\.com|oaistatic\.com/);

    expect(app.security.devCsp).toContain("ws://127.0.0.1:1420");
    expect(app.security.devCsp).not.toContain("ws://0.0.0.0");
  });

  it("grants only the renderer APIs the app calls", () => {
    const capability = capabilityJson as Capability;
    expect(capability.permissions).toEqual([
      "core:event:allow-listen",
      "core:event:allow-unlisten",
      "core:webview:allow-internal-toggle-devtools",
      "core:window:allow-internal-toggle-maximize",
      "core:window:allow-start-dragging",
      "core:window:allow-close",
      "core:window:allow-minimize",
      // The expanded workbench offers a native maximize toggle.
      "core:window:allow-toggle-maximize",
      // Window-mode switching stores the expanded size in logical px; the
      // renderer reads the monitor scale to convert onResized's physical px.
      "core:window:allow-scale-factor",
      "dialog:allow-open",
      "process:allow-restart",
    ]);
    expect(capability.permissions).not.toContain("core:default");
    expect(capability.permissions).not.toContain("updater:default");
    expect(capability.permissions).not.toContain("process:default");
  });
});
