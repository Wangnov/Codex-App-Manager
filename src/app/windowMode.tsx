import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { managerApi } from "../services/managerApi";
import type { WindowMode } from "../shared/types";

/** Remembered expanded size (logical px, JSON `{width,height}`). The *mode*
 *  itself is deliberately not persisted: the manager is an at-a-glance popover
 *  first, so every launch starts compact and the workbench is a per-session
 *  posture. */
const LS_SIZE_KEY = "cam.windowSize.expanded";

/** How long the stage fade-out gets before the native resize fires — matches
 *  the `#root` opacity transition in styles.css so the reflow happens behind
 *  an opaque veil. */
const SWITCH_FADE_MS = 150;

interface WindowModeCtx {
  mode: WindowMode;
  /** True while a switch is in flight (both directions). */
  switching: boolean;
  setMode: (mode: WindowMode) => void;
}

const Ctx = createContext<WindowModeCtx | null>(null);

function isTauri(): boolean {
  return (
    typeof window !== "undefined" &&
    Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
  );
}

function prefersReducedMotion(): boolean {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

function readStoredSize(): { width: number; height: number } | null {
  try {
    const raw = localStorage.getItem(LS_SIZE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as { width?: unknown; height?: unknown };
    if (typeof parsed.width !== "number" || typeof parsed.height !== "number") return null;
    if (!Number.isFinite(parsed.width) || !Number.isFinite(parsed.height)) return null;
    return { width: parsed.width, height: parsed.height };
  } catch {
    return null;
  }
}

function storeSize(size: { width: number; height: number }) {
  try {
    localStorage.setItem(LS_SIZE_KEY, JSON.stringify(size));
  } catch {
    // Size memory is a nicety; never let quota/serialization break a switch.
  }
}

const wait = (ms: number) => new Promise<void>((resolve) => window.setTimeout(resolve, ms));

export function WindowModeProvider({ children }: { children: ReactNode }) {
  const [mode, setModeState] = useState<WindowMode>("compact");
  const [switching, setSwitching] = useState(false);
  const modeRef = useRef(mode);
  modeRef.current = mode;
  const inFlight = useRef(false);
  // Last drag-resize not yet persisted (physical px), with the debounce timer
  // that will commit it. Flushed eagerly before leaving expanded (and on
  // unmount) so a collapse inside the debounce window can't eat the size.
  const pendingResize = useRef<{ width: number; height: number } | null>(null);
  const resizeTimer = useRef<number | null>(null);
  const scaleRef = useRef(1);

  const flushPendingSize = useCallback(() => {
    if (resizeTimer.current != null) {
      window.clearTimeout(resizeTimer.current);
      resizeTimer.current = null;
    }
    const pending = pendingResize.current;
    pendingResize.current = null;
    if (!pending) return;
    const factor = scaleRef.current > 0 ? scaleRef.current : 1;
    storeSize({ width: pending.width / factor, height: pending.height / factor });
  }, []);

  // Stamp the mode on <html> so styles.css can key the whole layout on it.
  useEffect(() => {
    document.documentElement.dataset.windowMode = mode;
    return () => {
      delete document.documentElement.dataset.windowMode;
    };
  }, [mode]);

  const setMode = useCallback(
    (next: WindowMode) => {
      if (inFlight.current || next === modeRef.current) return;
      inFlight.current = true;
      setSwitching(true);
      // Commit any drag-resize still sitting in the debounce window before the
      // native frame changes underneath it.
      flushPendingSize();
      const root = document.documentElement;
      const animate = !prefersReducedMotion();
      void (async () => {
        try {
          if (animate) {
            // Veil the stage, and give the fade time to complete before the
            // native window snaps to its new frame.
            root.dataset.windowModeSwitching = "true";
            await wait(SWITCH_FADE_MS);
          }
          const remembered = next === "expanded" ? readStoredSize() : null;
          const report = await managerApi.setWindowMode(next, remembered ?? undefined);
          if (report.mode === "expanded") {
            storeSize({ width: report.width, height: report.height });
          }
          // Stamp synchronously with the React state so the un-veil below
          // never shows a frame of the old layout in the new window frame.
          root.dataset.windowMode = report.mode;
          setModeState(report.mode);
          // The control that was activated (expand button / rail collapse)
          // unmounts with the layout swap while the view stays put, so the
          // App-level view-change focus restore never runs. Give keyboard
          // focus a definite landing: the rail's active item when expanding,
          // the visible view's page target when collapsing.
          window.requestAnimationFrame(() => {
            const landing =
              document.querySelector<HTMLElement>(".rail-item.active") ??
              document.querySelector<HTMLElement>(
                '[data-view] [data-page-focus]:not(:disabled)',
              ) ??
              document.querySelector<HTMLElement>(".topbar button, .navbar button");
            landing?.focus({ preventScroll: true });
          });
        } catch (cause) {
          console.warn("[window-mode] switch failed", cause);
        } finally {
          if (animate) {
            // Let the new layout commit before lifting the veil.
            window.requestAnimationFrame(() => {
              window.requestAnimationFrame(() => {
                delete root.dataset.windowModeSwitching;
              });
            });
          }
          inFlight.current = false;
          setSwitching(false);
        }
      })();
    },
    [flushPendingSize],
  );

  // Remember the user's expanded size as they drag-resize (native only; the
  // event payload is physical px, stored logical). Writes are debounced into
  // pendingResize and flushed by the timer, a mode switch, or unmount.
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    const win = getCurrentWindow();
    const refreshScale = () => {
      void win
        .scaleFactor()
        .then((scale) => {
          if (!disposed && scale > 0) scaleRef.current = scale;
        })
        .catch(() => undefined);
    };
    refreshScale();
    void win
      .onResized((event) => {
        if (modeRef.current !== "expanded" || inFlight.current) return;
        refreshScale();
        pendingResize.current = { ...event.payload };
        if (resizeTimer.current != null) window.clearTimeout(resizeTimer.current);
        resizeTimer.current = window.setTimeout(flushPendingSize, 400);
      })
      .then((dispose) => {
        if (disposed) dispose();
        else unlisten = dispose;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      flushPendingSize();
      unlisten?.();
    };
  }, [flushPendingSize]);

  const value = useMemo(() => ({ mode, switching, setMode }), [mode, switching, setMode]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

/** Nullable accessor: chrome that merely *reacts* to the mode (expand button,
 *  rail) renders nothing without a provider, so components stay renderable in
 *  isolation (tests, storybook-style harnesses). */
export function useWindowModeOptional(): WindowModeCtx | null {
  return useContext(Ctx);
}

export function useWindowMode(): WindowModeCtx {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useWindowMode must be used within WindowModeProvider");
  return ctx;
}
