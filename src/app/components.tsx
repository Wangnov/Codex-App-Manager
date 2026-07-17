import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";

import { errorMessage, managerApi } from "../services/managerApi";
import type { FailureSurface } from "./errorCopy";
import { Icon, type IconName, CodexMark } from "./icons";
import { useI18n } from "./i18n";
import { Sheet } from "./Sheet";
import { useWindowModeOptional } from "./windowMode";

const FRONTEND_READY_EVENT = "cam:frontend-readiness";

type FrontendReadiness = { generation: number; token: string };

function frontendReadiness(): FrontendReadiness | null {
  const readiness = (
    window as typeof window & { __CAM_FRONTEND_READY__?: unknown }
  ).__CAM_FRONTEND_READY__;
  if (!readiness || typeof readiness !== "object") return null;
  const { generation, token } = readiness as Partial<FrontendReadiness>;
  if (!Number.isSafeInteger(generation) || (generation ?? 0) <= 0) return null;
  if (typeof token !== "string" || !token.trim()) return null;
  return { generation: generation as number, token };
}

function isTauri(): boolean {
  return (
    typeof window !== "undefined" &&
    Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
  );
}

/** Ask to close the window. In the desktop app this hits the CloseRequested
 *  guard in lib.rs, which raises the confirm dialog (app://confirm-quit) when
 *  关闭前确认 is on, or exits when it's off — the same path Alt+F4 / Cmd+Q take.
 *  In the browser there's no backend, so emulate the guard locally. */
function requestQuit() {
  if (isTauri()) {
    void getCurrentWindow().close();
  } else {
    void managerApi.getSettings().then((s) => {
      if (s.confirmClose) window.dispatchEvent(new Event("cam:confirm-quit"));
    });
  }
}

/** Self-drawn close control (the window is frameless). It only *requests* a
 *  close; whether to confirm first is decided by the backend guard, so the ✕,
 *  Alt+F4 and Cmd+Q all behave identically. */
function CloseButton() {
  const { t } = useI18n();
  return (
    <button className="winclose" title={t("nav.close")} onClick={requestQuit}>
      <Icon name="close" />
    </button>
  );
}

/** Self-drawn minimize control, sitting left of the close button (the window is
 *  frameless, so it's self-drawn too). Only meaningful in the desktop app; the
 *  browser preview has no window to minimize, so it's a no-op there. */
function MinimizeButton() {
  const { t } = useI18n();
  return (
    <button
      className="winmin"
      title={t("nav.minimize")}
      onClick={() => {
        if (isTauri()) void getCurrentWindow().minimize();
      }}
    >
      <Icon name="minimize" />
    </button>
  );
}

/** Expand-to-workbench control, sitting with the window controls while the
 *  window is compact. The reverse lives on the rail (its 收起 item), so this
 *  renders nothing once expanded — and nothing at all without a
 *  WindowModeProvider (isolated component tests). */
function ExpandButton() {
  const { t } = useI18n();
  const windowMode = useWindowModeOptional();
  if (!windowMode || windowMode.mode !== "compact") return null;
  return (
    <button
      className="iconbtn"
      title={t("nav.expand")}
      disabled={windowMode.switching}
      onClick={() => windowMode.setMode("expanded")}
    >
      <Icon name="expand" />
    </button>
  );
}

/** Native maximize toggle — expanded only (compact is a fixed-size popover).
 *  Collapsing from a maximized frame is handled backend-side (set_window_mode
 *  un-maximizes first). */
function MaximizeButton() {
  const { t } = useI18n();
  const windowMode = useWindowModeOptional();
  if (windowMode?.mode !== "expanded") return null;
  return (
    <button
      className="iconbtn winmax"
      title={t("nav.maximize")}
      onClick={() => {
        if (isTauri()) void getCurrentWindow().toggleMaximize();
      }}
    >
      <Icon name="maximize" />
    </button>
  );
}

/** The one close-confirm dialog. Raised by the backend close/exit guard (which
 *  covers the ✕, Alt+F4 and Cmd+Q alike) via app://confirm-quit, or by the
 *  browser-preview fallback. Mounted once at the app root so it overlays
 *  whichever view is showing; confirming asks the backend to actually exit.
 *
 *  When the backend is mid point-of-no-return install (`app://quit-blocked`),
 *  a different sheet explains why quit is refused. */
export function QuitConfirm() {
  const { t, lang } = useI18n();
  const [open, setOpen] = useState(false);
  const [blockedCode, setBlockedCode] = useState<string | null>(null);
  const [blockedFallback, setBlockedFallback] = useState<string | null>(null);
  const [listenersReady, setListenersReady] = useState(false);
  const titleId = useId();
  const bodyId = useId();

  useEffect(() => {
    let disposed = false;
    let retryTimer: number | null = null;
    let attempt = 0;
    const registered = new Set<() => void>();
    const releaseListeners = (listeners = [...registered]) => {
      listeners.forEach((unlisten) => {
        if (!registered.delete(unlisten)) return;
        unlisten();
      });
    };
    const registerListeners = async () => {
      attempt += 1;
      const currentAttempt: Array<() => void> = [];
      try {
        const confirmUnlisten = await listen("app://confirm-quit", () => {
          setBlockedCode(null);
          setBlockedFallback(null);
          setOpen(true);
        });
        registered.add(confirmUnlisten);
        currentAttempt.push(confirmUnlisten);
        if (disposed) {
          releaseListeners(currentAttempt);
          return;
        }

        const blockedUnlisten = await listen<{ reasonCode?: string; reason?: string }>(
          "app://quit-blocked",
          (event) => {
            const code =
              typeof event.payload?.reasonCode === "string" && event.payload.reasonCode.trim()
                ? event.payload.reasonCode
                : "busy";
            const fallback =
              typeof event.payload?.reason === "string" && event.payload.reason.trim()
                ? event.payload.reason
                : null;
            setBlockedCode(code);
            setBlockedFallback(fallback);
            setOpen(true);
          },
        );
        registered.add(blockedUnlisten);
        currentAttempt.push(blockedUnlisten);
        if (disposed) {
          releaseListeners(currentAttempt);
          return;
        }
        setListenersReady(true);
      } catch (cause) {
        releaseListeners(currentAttempt);
        if (disposed) return;
        const message = errorMessage(cause);
        if (attempt === 1) {
          console.error("[native-shell] quit listener registration failed", cause);
          void managerApi.reportFrontendError({
            kind: "native-shell-listeners",
            message,
            stack: cause instanceof Error ? cause.stack ?? null : null,
            componentStack: null,
          });
        } else {
          console.warn(`[native-shell] quit listener retry ${attempt} failed`, cause);
        }
        const delay = Math.min(5000, 250 * 2 ** Math.min(attempt - 1, 5));
        retryTimer = window.setTimeout(() => void registerListeners(), delay);
      }
    };
    // Browser previews have no Tauri event bridge. Their close-confirm path is
    // the local cam:confirm-quit event below, so treating a missing bridge as a
    // transient native failure would only create a permanent retry loop.
    if (isTauri()) void registerListeners();
    const onWeb = () => {
      setBlockedCode(null);
      setBlockedFallback(null);
      setOpen(true);
    };
    window.addEventListener("cam:confirm-quit", onWeb);
    return () => {
      disposed = true;
      if (retryTimer != null) window.clearTimeout(retryTimer);
      releaseListeners();
      window.removeEventListener("cam:confirm-quit", onWeb);
    };
  }, []);

  useEffect(() => {
    if (!listenersReady) return;
    let cancelled = false;
    let retryTimer: number | null = null;
    let attempt = 0;
    let attemptKey: string | null = null;
    let inFlight = false;
    let rerun = false;
    const announceReady = async () => {
      if (inFlight) {
        rerun = true;
        return;
      }
      const readiness = frontendReadiness();
      if (!readiness) return;
      const key = `${readiness.generation}:${readiness.token}`;
      if (attemptKey !== key) {
        attemptKey = key;
        attempt = 0;
      }
      attempt += 1;
      inFlight = true;
      try {
        await managerApi.frontendReady(lang, readiness.generation, readiness.token);
      } catch (cause) {
        if (cancelled) return;
        const message = errorMessage(cause);
        if (attempt === 1) {
          console.error("[native-shell] frontend-ready handshake failed; retrying", cause);
          void managerApi.reportFrontendError({
            kind: "native-shell-ready",
            message,
            stack: cause instanceof Error ? cause.stack ?? null : null,
            componentStack: null,
          });
        } else {
          console.warn(`[native-shell] frontend-ready retry ${attempt} failed`, cause);
        }
        const delay = Math.min(5000, 250 * 2 ** Math.min(attempt - 1, 5));
        retryTimer = window.setTimeout(() => void announceReady(), delay);
      } finally {
        inFlight = false;
        if (rerun && !cancelled) {
          rerun = false;
          if (retryTimer != null) {
            window.clearTimeout(retryTimer);
            retryTimer = null;
          }
          void announceReady();
        }
      }
    };
    const onToken = () => {
      if (cancelled) return;
      if (retryTimer != null) {
        window.clearTimeout(retryTimer);
        retryTimer = null;
      }
      void announceReady();
    };
    window.addEventListener(FRONTEND_READY_EVENT, onToken);
    void announceReady();
    return () => {
      cancelled = true;
      if (retryTimer != null) window.clearTimeout(retryTimer);
      window.removeEventListener(FRONTEND_READY_EVENT, onToken);
    };
  }, [lang, listenersReady]);

  const blocked = blockedCode !== null;
  const blockedBody = !blocked
    ? t("close.confirm.body")
    : blockedCode === "committing"
      ? t("close.blocked.reason.committing")
      : blockedCode === "finishing"
        ? t("close.blocked.reason.finishing")
        : blockedCode === "other-process"
          ? t("close.blocked.reason.otherProcess")
          : blockedFallback || t("close.blocked.body");

  return (
    <Sheet
      open={open}
      onDismiss={() => {
        setOpen(false);
        setBlockedCode(null);
        setBlockedFallback(null);
      }}
      scrimClass="quit-scrim"
      labelledBy={titleId}
      describedBy={bodyId}
      initialFocus="dismiss"
    >
      <Ring icon="info" variant="amber" />
      <h3 id={titleId}>
        {blocked ? t("close.blocked.title") : t("close.confirm.title")}
      </h3>
      <p id={bodyId}>{blocked ? blockedBody : t("close.confirm.body")}</p>
      <div className="row2 sheet-actions">
        {blocked ? (
          <button
            className="btn primary"
            onClick={() => {
              setOpen(false);
              setBlockedCode(null);
              setBlockedFallback(null);
            }}
          >
            {t("close.blocked.ok")}
          </button>
        ) : (
          <>
            <button className="btn ghost" onClick={() => setOpen(false)}>
              {t("confirm.cancel")}
            </button>
            <button
              className="btn primary"
              onClick={() => {
                setOpen(false);
                void managerApi.confirmQuit();
              }}
            >
              {t("close.confirm.ok")}
            </button>
          </>
        )}
      </div>
    </Sheet>
  );
}

// The window is frameless, so the bar drags it. data-app-drag (see
// windowDrag.ts) triggers on the element actually clicked, so every non-button
// element in the bar carries it; the buttons deliberately don't (they stay
// clickable). Double-clicking a marked element zooms the expanded workbench.
export function TopBar({ children }: { children?: ReactNode }) {
  const { t } = useI18n();
  return (
    <div className="topbar" data-app-drag>
      <div className="mark" data-app-drag>
        <CodexMark />
      </div>
      <div className="wordmark" data-app-drag>
        {t("app.name")}
      </div>
      <div className="spacer" data-app-drag />
      {/* First interactive child is the preferred home focus target after a
          view transition (see App focus restore + data-page-focus). */}
      {children}
      <ExpandButton />
      <MaximizeButton />
      <MinimizeButton />
      <CloseButton />
    </div>
  );
}

export function NavBar({
  title,
  onBack,
  disableBack = false,
  children,
}: {
  title: string;
  onBack: () => void;
  disableBack?: boolean;
  children?: ReactNode;
}) {
  const { t } = useI18n();
  const backRef = useRef<HTMLButtonElement>(null);

  // Sub-views replace the previous page without a focus handoff; when focus
  // lands on <body>, put it on the back control so keyboard users aren't stuck.
  useLayoutEffect(() => {
    if (disableBack) return;
    const active = document.activeElement;
    if (active && active !== document.body && active !== document.documentElement) {
      return;
    }
    backRef.current?.focus({ preventScroll: true });
  }, [disableBack]);

  return (
    <div className="navbar" data-app-drag>
      <button
        ref={backRef}
        className="navback"
        data-page-focus
        onClick={onBack}
        disabled={disableBack}
      >
        <Icon name="back" />
        {t("nav.back")}
      </button>
      <div className="navtitle" data-app-drag>
        {title}
      </div>
      <div className="spacer" style={{ flex: 1 }} data-app-drag />
      {children}
      <ExpandButton />
      <MaximizeButton />
      <MinimizeButton />
      <CloseButton />
    </div>
  );
}

export type RingVariant = "accent" | "success" | "amber" | "muted" | "danger";

export function Ring({
  icon,
  variant = "accent",
  spin = false,
  className = "",
}: {
  icon: IconName;
  variant?: RingVariant;
  spin?: boolean;
  className?: string;
}) {
  const v = variant === "accent" ? "" : variant;
  return (
    <div className={`ring ${v} ${spin ? "spin" : ""} ${className}`}>
      <Icon name={icon} />
    </div>
  );
}

/** The "检查失败" hero, shared by both platform homes. Selects title/body by
 *  stable failure code (never by string-matching engine text). Raw diagnostics
 *  stay one tap away behind 查看详情 for bug reports.
 *
 *  `role="alert"` wraps only the spoken copy — not the disclosure control — so
 *  expanding details doesn't re-announce the whole interactive tree. */
export function ErrorHero({ failure }: { failure: FailureSurface | null }) {
  const { t } = useI18n();
  const [showDetails, setShowDetails] = useState(false);
  const network = failure?.code === "network" || failure?.code === "timeout";
  // Connectivity keeps the short network sub under a calm title; other codes
  // use the classified localized message as the body.
  const body = network
    ? t("home.error.network.sub")
    : (failure?.message ?? t("home.error.sub"));
  return (
    <>
      <Ring icon="alert" variant="danger" />
      <div role="alert" aria-live="assertive">
        <div className="headline">
          {t(network ? "home.error.network.title" : "home.error.title")}
        </div>
        <div className="desc">{body}</div>
      </div>
      {failure?.detail ? (
        <>
          <button
            type="button"
            className={`errdetails-toggle${showDetails ? " open" : ""}`}
            aria-expanded={showDetails}
            onClick={() => setShowDetails((v) => !v)}
          >
            {showDetails ? t("home.error.hideDetails") : t("home.error.details")}
          </button>
          {/* Grow the raw diagnostic in via grid-rows (same accordion technique
              as .schedule-panel) instead of letting it pop in/out. */}
          <div
            className={`errdetails-panel${showDetails ? " open" : ""}`}
            aria-hidden={!showDetails}
            inert={showDetails ? undefined : true}
          >
            <div className="errdetails-panel-inner">
              <pre className="errdetails">{failure.detail}</pre>
            </div>
          </div>
        </>
      ) : null}
    </>
  );
}

/** Inline status/error strip used across Home, Settings, Uninstall, etc.
 *  Live-region attributes sit on the text span so action buttons (retry) are
 *  not part of the announced tree. */
export function StatusBanner({
  tone,
  children,
  action,
  icon,
}: {
  tone: "err" | "info" | "ok" | "warn";
  children: ReactNode;
  action?: ReactNode;
  icon?: IconName;
}) {
  const isError = tone === "err";
  const resolvedIcon: IconName =
    icon ?? (isError ? "alert" : tone === "ok" ? "check" : "info");
  return (
    <div className={`banner ${tone}`}>
      <Icon name={resolvedIcon} />
      <span role={isError ? "alert" : "status"} aria-live={isError ? "assertive" : "polite"}>
        {children}
      </span>
      {action}
    </div>
  );
}

/** Action-path failure banner: localized primary message + optional raw detail
 *  disclosure. Used by Home/WinHome install/update/launch paths. */
export function FailureBanner({ failure }: { failure: FailureSurface }) {
  const { t } = useI18n();
  const [showDetails, setShowDetails] = useState(false);
  return (
    <div className="banner err failure-banner">
      <Icon name="alert" />
      <div className="failure-banner-body">
        <span role="alert" aria-live="assertive">
          {failure.message}
        </span>
        {failure.detail ? (
          <>
            <button
              type="button"
              className={`errdetails-toggle failure-banner-toggle${showDetails ? " open" : ""}`}
              aria-expanded={showDetails}
              onClick={() => setShowDetails((v) => !v)}
            >
              {showDetails ? t("home.error.hideDetails") : t("home.error.details")}
            </button>
            {showDetails ? (
              <pre className="errdetails failure-banner-detail">{failure.detail}</pre>
            ) : null}
          </>
        ) : null}
      </div>
    </div>
  );
}

/** Update-outcome strip shown after a perform/install completes. Unlike the
 *  inline `.banner` notes it owns its lifecycle: it can be dismissed by hand
 *  (✕), and a clean success auto-dismisses after `autoDismissMs`. On exit it
 *  collapses its own height (grid-rows) so the content below glides up instead
 *  of snapping. Carries one rare accent of color (the badge) and a tabular
 *  title, so a version bump like "26.602.40724 → 26.602.71036" reads cleanly. */
export function ResultBanner({
  tone,
  title,
  detail,
  autoDismissMs,
  onClose,
}: {
  tone: "ok" | "err";
  title: ReactNode;
  detail?: ReactNode;
  autoDismissMs?: number;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const [leaving, setLeaving] = useState(false);

  // Clean, relaunched success fades itself out; everything else stays until the
  // user dismisses it (a rollback or a "launch manually" note must be read).
  useEffect(() => {
    if (!autoDismissMs) return;
    const id = window.setTimeout(() => setLeaving(true), autoDismissMs);
    return () => window.clearTimeout(id);
  }, [autoDismissMs]);

  // Once leaving, let the collapse/fade play, then actually unmount. Honor
  // reduced-motion by unmounting immediately (the CSS drops the transition too).
  useEffect(() => {
    if (!leaving) return;
    const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    const id = window.setTimeout(onClose, reduce ? 0 : 340);
    return () => window.clearTimeout(id);
  }, [leaving, onClose]);

  return (
    <div className={`resultbar-slot${leaving ? " leaving" : ""}`}>
      <div className={`resultbar ${tone}`}>
        <span className="rb-badge" aria-hidden="true">
          <Icon name={tone === "ok" ? "check" : "alert"} />
        </span>
        <span
          className="rb-text"
          role={tone === "err" ? "alert" : "status"}
          aria-live={tone === "err" ? "assertive" : "polite"}
        >
          <span className="rb-title">{title}</span>
          {detail ? <span className="rb-detail">{detail}</span> : null}
        </span>
        <button className="rb-close" title={t("nav.close")} onClick={() => setLeaving(true)}>
          <Icon name="close" />
        </button>
      </div>
    </div>
  );
}

/** Shared arrow-key contract for the button-based radio groups (Segmented,
 *  the settings source / install-mode lists, the language grid): arrows move
 *  AND select, wrapping at the ends; roving tabindex keeps Tab a single stop.
 *  Horizontal arrows follow the group's writing direction so RTL (Arabic)
 *  isn't mirrored. Returns the key to select (focus moves with it via the
 *  roving tabIndex render), or null for keys the group doesn't own. Ignores
 *  events from non-radio targets so inputs inside the group keep their caret
 *  keys. */
export function radioNavTarget(
  keys: string[],
  value: string,
  event: KeyboardEvent<HTMLElement>,
  container: HTMLElement | null,
): string | null {
  const { key } = event;
  if (key !== "ArrowLeft" && key !== "ArrowRight" && key !== "ArrowUp" && key !== "ArrowDown") {
    return null;
  }
  const target = event.target as HTMLElement | null;
  if (target && target.getAttribute("role") !== "radio") return null;
  event.preventDefault();
  // Only the horizontal pair mirrors with the writing direction; Up/Down are
  // direction-agnostic.
  const rtl = container ? getComputedStyle(container).direction === "rtl" : false;
  const horizontal = key === "ArrowLeft" || key === "ArrowRight";
  const forward = horizontal ? (key === "ArrowRight") !== rtl : key === "ArrowDown";
  const idx = keys.indexOf(value);
  const next = keys[(idx + (forward ? 1 : keys.length - 1)) % keys.length];
  // Focus the radio that is about to become checked. It carries tabIndex=-1
  // until the re-render lands, so look it up positionally.
  const radios = container?.querySelectorAll<HTMLElement>('[role="radio"]');
  radios?.[keys.indexOf(next)]?.focus();
  return next;
}

export interface SegmentedItem {
  key: string;
  label: ReactNode;
}

/** Segmented control whose selection is a single pill that *slides* between
 *  options instead of blinking in place. The pill carries the chrome (gradient +
 *  border + elevation) the selected button used to paint; JS writes the active
 *  button's offsetLeft / offsetWidth onto it and CSS owns the travel. Shared by
 *  the theme, check-frequency and proxy choosers. */
export function Segmented({
  items,
  value,
  onChange,
  ariaLabel,
}: {
  items: SegmentedItem[];
  value: string;
  onChange: (key: string) => void;
  ariaLabel?: string;
}) {
  const barRef = useRef<HTMLDivElement>(null);
  const pillRef = useRef<HTMLSpanElement>(null);
  // The pill snaps (no slide) into place on first paint; only a *selection*
  // change should animate. Tracks whether we've positioned it at least once.
  const placed = useRef(false);

  const place = useCallback((animate: boolean) => {
    const bar = barRef.current;
    const pill = pillRef.current;
    if (!bar || !pill) return;
    const active = bar.querySelector<HTMLElement>('[aria-checked="true"]');
    // offsetWidth is 0 while collapsed (e.g. inside a closed schedule panel) or
    // under jsdom — skip so we don't pin the pill to a zero-width ghost.
    if (!active || active.offsetWidth === 0) return;
    const write = () => {
      pill.style.transform = `translateX(${active.offsetLeft}px)`;
      pill.style.width = `${active.offsetWidth}px`;
    };
    if (animate) {
      write();
    } else {
      // Suspend the transition, write, force a reflow, restore — so the pill
      // lands at the new position before any tween can run.
      const prev = pill.style.transition;
      pill.style.transition = "none";
      write();
      void pill.offsetWidth;
      pill.style.transition = prev;
    }
  }, []);

  // Slide on selection change; snap the very first time.
  useLayoutEffect(() => {
    place(placed.current);
    placed.current = true;
  }, [value, place]);

  // Re-snap (no slide) when the bar's box changes — a language switch rewrites
  // every label's width, and the frequency segment is revealed from a collapsed
  // panel. ResizeObserver catches both without animating a non-selection move.
  useEffect(() => {
    const bar = barRef.current;
    if (!bar || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => place(false));
    ro.observe(bar);
    return () => ro.disconnect();
  }, [place]);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const next = radioNavTarget(
      items.map((item) => item.key),
      value,
      event,
      barRef.current,
    );
    if (next === null) return;
    onChange(next);
  };

  return (
    // Roving-tabindex radiogroup: focus + Tab live on the child radios, so the
    // group itself is correctly non-focusable (WAI-ARIA APG). The arrow-key
    // handler reads keydown bubbling up from the focused radio.
    // eslint-disable-next-line jsx-a11y/interactive-supports-focus
    <div className="seg" ref={barRef} role="radiogroup" aria-label={ariaLabel} onKeyDown={onKeyDown}>
      <span className="seg-pill" aria-hidden="true" ref={pillRef} />
      {items.map((item) => (
        <button
          key={item.key}
          role="radio"
          aria-checked={value === item.key}
          tabIndex={value === item.key ? 0 : -1}
          onClick={() => onChange(item.key)}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}

export function Toggle({
  checked,
  onChange,
  disabled = false,
  ariaLabelledBy,
}: {
  checked: boolean;
  onChange?: (v: boolean) => void;
  disabled?: boolean;
  /** id of the visible row title — the switch itself renders no text, so
   *  without this a screen reader announces a nameless "switch, on". */
  ariaLabelledBy?: string;
}) {
  return (
    <button
      className="toggle"
      role="switch"
      aria-checked={checked}
      aria-labelledby={ariaLabelledBy}
      disabled={disabled}
      onClick={() => onChange?.(!checked)}
    />
  );
}
