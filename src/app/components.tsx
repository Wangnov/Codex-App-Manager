import { useEffect, useState, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";

import { managerApi } from "../services/managerApi";
import { Icon, type IconName, CodexMark } from "./icons";
import { useI18n } from "./i18n";

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

/** The one close-confirm dialog. Raised by the backend close/exit guard (which
 *  covers the ✕, Alt+F4 and Cmd+Q alike) via app://confirm-quit, or by the
 *  browser-preview fallback. Mounted once at the app root so it overlays
 *  whichever view is showing; confirming asks the backend to actually exit. */
export function QuitConfirm() {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);

  useEffect(() => {
    let un = () => {};
    void listen("app://confirm-quit", () => setOpen(true))
      .then((f) => (un = f))
      .catch(() => undefined);
    const onWeb = () => setOpen(true);
    window.addEventListener("cam:confirm-quit", onWeb);
    return () => {
      un();
      window.removeEventListener("cam:confirm-quit", onWeb);
    };
  }, []);

  if (!open) return null;
  return (
    <div className="quit-scrim" onClick={() => setOpen(false)}>
      <div className="sheet" onClick={(e) => e.stopPropagation()}>
        <Ring icon="info" variant="amber" />
        <h3>{t("close.confirm.title")}</h3>
        <p>{t("close.confirm.body")}</p>
        <div className="row2">
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
        </div>
      </div>
    </div>
  );
}

// The window is frameless, so the bar drags it. tauri's data-tauri-drag-region
// triggers on the element actually clicked, so every non-button element in the
// bar carries it; the buttons deliberately don't (they stay clickable).
export function TopBar({ children }: { children?: ReactNode }) {
  const { t } = useI18n();
  return (
    <div className="topbar" data-tauri-drag-region>
      <div className="mark" data-tauri-drag-region>
        <CodexMark />
      </div>
      <div className="wordmark" data-tauri-drag-region>
        {t("app.name")}
      </div>
      <div className="spacer" data-tauri-drag-region />
      {children}
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
  return (
    <div className="navbar" data-tauri-drag-region>
      <button className="navback" onClick={onBack} disabled={disableBack}>
        <Icon name="back" />
        {t("nav.back")}
      </button>
      <div className="navtitle" data-tauri-drag-region>
        {title}
      </div>
      <div className="spacer" style={{ flex: 1 }} data-tauri-drag-region />
      {children}
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
      <div className={`resultbar ${tone}`} role="status" aria-live="polite">
        <span className="rb-badge" aria-hidden="true">
          <Icon name={tone === "ok" ? "check" : "alert"} />
        </span>
        <span className="rb-text">
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

export function Toggle({
  checked,
  onChange,
  disabled = false,
}: {
  checked: boolean;
  onChange?: (v: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <button
      className="toggle"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={() => onChange?.(!checked)}
    />
  );
}
