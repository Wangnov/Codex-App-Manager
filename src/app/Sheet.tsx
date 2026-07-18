import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

import { useFocusTrap } from "./useFocusTrap";

// Upper bound on the exit. Unmount is driven by the frame's animationend
// (the `.is-closing` exit is a CSS animation); this only needs to be ≥ the
// longest exit in styles.css, as the safety net for environments where the
// animation never fires (jsdom, display:none ancestors).
const CLOSE_FALLBACK_MS = 400;

function prefersReducedMotion(): boolean {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

export function Sheet({
  open,
  onDismiss,
  dismissable = true,
  scrimClass = "scrim",
  labelledBy,
  describedBy,
  initialFocus = "dismiss",
  centeredInExpanded = false,
  children,
}: {
  open: boolean;
  onDismiss?: () => void;
  dismissable?: boolean;
  scrimClass?: "scrim" | "quit-scrim";
  labelledBy?: string;
  describedBy?: string;
  initialFocus?: "first" | "primary" | "dismiss" | "container";
  /** In the expanded workbench, render as a centered modal instead of a bottom
   *  sheet. Ignored in compact mode (always a sheet there). */
  centeredInExpanded?: boolean;
  children: ReactNode;
}) {
  const sheetRef = useRef<HTMLDivElement>(null);
  const frameRef = useRef<HTMLDivElement>(null);
  // Keep the sheet in the DOM through its exit. `mounted` only *lags* `open` on
  // close: presence is `open || mounted`, so opening shows the node immediately
  // (the focus trap can grab focus the same render) while closing keeps the same
  // node around. The closing flag is derived live as `!open`, so the frame `open`
  // flips false the *existing* node gains `.is-closing` and the exit transition
  // animates from its current state — unmount happens on the frame's own
  // animationend (with a timeout safety net), so the CSS duration can change
  // without touching this file.
  // (Driving `.is-closing` off an effect-set state instead would unmount the node
  // before the class lands, so the exit would never play.)
  const [mounted, setMounted] = useState(open);

  useEffect(() => {
    if (open) {
      setMounted(true);
      return;
    }
    if (!mounted) return; // already gone
    if (prefersReducedMotion()) {
      setMounted(false);
      return;
    }
    const frame = frameRef.current;
    const finish = () => setMounted(false);
    const onAnimationEnd = (event: AnimationEvent) => {
      if (event.target === frame) finish();
    };
    frame?.addEventListener("animationend", onAnimationEnd);
    const id = window.setTimeout(finish, CLOSE_FALLBACK_MS);
    return () => {
      frame?.removeEventListener("animationend", onAnimationEnd);
      window.clearTimeout(id);
    };
  }, [open, mounted]);

  // Ignore dismissals once the exit is playing (`open` already false). The trap
  // is `active` only while open: opening arms it (focus into the sheet), closing
  // disarms it and hands focus back to the opener immediately, so the exit just
  // plays out as a visual with no focus held in the unmounting sheet.
  const openRef = useRef(open);
  openRef.current = open;
  const dismiss = useCallback(() => {
    if (dismissable && openRef.current) onDismiss?.();
  }, [dismissable, onDismiss]);
  useFocusTrap(sheetRef, { onEsc: dismiss, initialFocus, active: open });

  if (!open && !mounted) {
    return null;
  }

  const closing = !open;
  return (
    // Backdrop click-to-dismiss is a mouse convenience; the keyboard path is Esc
    // (handled by the focus trap's onEsc). The dialog itself carries the
    // interactive semantics (role=dialog + aria-modal), so the scrim is inert
    // chrome, not a control.
    // eslint-disable-next-line jsx-a11y/no-static-element-interactions, jsx-a11y/click-events-have-key-events
    <div
      className={`${scrimClass}${centeredInExpanded ? " sheet-centerable" : ""}${closing ? " is-closing" : ""}`}
      onClick={dismiss}
    >
      {/* onClick here is pure event plumbing — it stops a click INSIDE the sheet
          from bubbling to the backdrop-dismiss handler; it is not a control. */}
      {/* eslint-disable-next-line jsx-a11y/no-static-element-interactions, jsx-a11y/click-events-have-key-events */}
      <div
        ref={frameRef}
        className={`sheet-frame${centeredInExpanded ? " sheet-centerable" : ""}${closing ? " is-closing" : ""}`}
        onClick={(event) => event.stopPropagation()}
      >
        <div
          ref={sheetRef}
          className="sheet"
          role="dialog"
          aria-modal="true"
          aria-labelledby={labelledBy}
          aria-describedby={describedBy}
        >
          {children}
        </div>
      </div>
    </div>
  );
}
