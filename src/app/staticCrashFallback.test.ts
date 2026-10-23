import { fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  disposeStaticCrashPolicy,
  installStaticCrashPolicy,
  renderStaticCrashFallback,
  shouldBlockStaticCrashShortcut,
} from "./staticCrashFallback";

describe("renderStaticCrashFallback", () => {
  afterEach(() => {
    disposeStaticCrashPolicy();
    document.body.innerHTML = '<div id="root"></div>';
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    vi.restoreAllMocks();
  });

  it("renders and logs with dependency-free APIs when createRoot has not run", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const invoke = vi.fn().mockResolvedValue(undefined);
    (
      window as typeof window & { __TAURI_INTERNALS__?: StaticTauriInternalsForTest }
    ).__TAURI_INTERNALS__ = { invoke };
    document.getElementById("root")?.remove();

    const surface = renderStaticCrashFallback(new Error("createRoot failed"), document, true);

    expect(surface.dataset.staticCrash).toBe("true");
    expect(surface).toHaveTextContent("createRoot failed");
    expect(document.getElementById("root")).toContainElement(surface);
    // CSSOM may expand the background shorthand; assert the opaque color itself.
    expect(surface).toHaveStyle({ backgroundColor: "#17171d", color: "#f7f6fa" });
    expect(surface.querySelectorAll("button")).toHaveLength(2);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "log_frontend_error",
        expect.objectContaining({
          payload: expect.objectContaining({ kind: "bootstrap", message: "createRoot failed" }),
        }),
      ),
    );
    expect(consoleError).toHaveBeenCalled();
  });

  it("blocks context menus across the whole document, including outside #root", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    renderStaticCrashFallback(new Error("bootstrap failed"), document, true);
    const outside = document.createElement("aside");
    document.body.appendChild(outside);
    const menu = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });

    outside.dispatchEvent(menu);

    expect(menu.defaultPrevented).toBe(true);
  });

  it.each([
    ["F5", {}],
    ["F12", {}],
    ["r", { ctrlKey: true }],
    ["p", { metaKey: true }],
    ["R", { metaKey: true, shiftKey: true }],
    ["i", { ctrlKey: true, shiftKey: true }],
    ["j", { metaKey: true, altKey: true }],
    ["BrowserBack", {}],
    ["ArrowLeft", { altKey: true }],
    ["х", { code: "BracketLeft", metaKey: true }],
  ] as const)("blocks release browser shortcut %s on the fallback document", (key, init) => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    renderStaticCrashFallback(new Error("bootstrap failed"), document, true);
    const event = new KeyboardEvent("keydown", {
      key,
      bubbles: true,
      cancelable: true,
      ...init,
    });

    document.body.dispatchEvent(event);

    expect(event.defaultPrevented).toBe(true);
    expect(shouldBlockStaticCrashShortcut(event)).toBe(true);
  });

  it("blocks mouse back/forward at every event phase used by WebViews", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    renderStaticCrashFallback(new Error("bootstrap failed"), document, true);

    for (const type of ["mousedown", "mouseup", "auxclick"]) {
      const event = new MouseEvent(type, { button: 3, bubbles: true, cancelable: true });
      document.body.dispatchEvent(event);
      expect(event.defaultPrevented, type).toBe(true);
    }
  });

  it("keeps browser debugging behavior in development mode", () => {
    installStaticCrashPolicy(document, false);
    const menu = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    const reload = new KeyboardEvent("keydown", {
      key: "r",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });

    document.body.dispatchEvent(menu);
    document.body.dispatchEvent(reload);

    expect(menu.defaultPrevented).toBe(false);
    expect(reload.defaultPrevented).toBe(false);
  });

  it("quits through the backend phase policy and displays a serialized refusal", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const invoke = vi.fn((command: string) => {
      if (command === "confirm_quit") {
        return Promise.reject({
          code: "operation_not_interruptible",
          message: "install is committing",
        });
      }
      return Promise.resolve();
    });
    (
      window as typeof window & { __TAURI_INTERNALS__?: StaticTauriInternalsForTest }
    ).__TAURI_INTERNALS__ = { invoke };
    const surface = renderStaticCrashFallback(new Error("bootstrap failed"), document, true);

    fireEvent.click(Array.from(surface.querySelectorAll("button"))[1]);

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("confirm_quit", undefined));
    await waitFor(() => expect(surface).toHaveTextContent("install is committing"));
    expect(surface).not.toHaveTextContent("[object Object]");
  });

  it("still renders and surfaces quit failure when the raw IPC bridge throws synchronously", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const invoke = vi.fn(() => {
      throw new Error("bridge failed synchronously");
    });
    (
      window as typeof window & { __TAURI_INTERNALS__?: StaticTauriInternalsForTest }
    ).__TAURI_INTERNALS__ = { invoke };

    const surface = renderStaticCrashFallback(new Error("bootstrap failed"), document, true);
    fireEvent.click(Array.from(surface.querySelectorAll("button"))[1]);

    await waitFor(() => expect(surface).toHaveTextContent("bridge failed synchronously"));
  });
});

type StaticTauriInternalsForTest = {
  invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
};
