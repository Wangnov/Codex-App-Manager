import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

import { useFocusTrap } from "./useFocusTrap";

// Keep in sync with the longest `.sheet-frame.is-closing` exit transition in
// styles.css — once it elapses the sheet is actually unmounted.
const CLOSE_MS = 240;

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
  children,
}: {
  open: boolean;
  onDismiss?: () => void;
  dismissable?: boolean;
  scrimClass?: "scrim" | "quit-scrim";
  labelledBy?: string;
  describedBy?: string;
  initialFocus?: "first" | "primary" | "dismiss" | "container";
  children: ReactNode;
}) {
  const sheetRef = useRef<HTMLDivElement>(null);
  // Keep the sheet in the DOM through its exit. `mounted` only *lags* `open` on
  // close: presence is `open || mounted`, so opening shows the node immediately
  // (the focus trap can grab focus the same render) while closing keeps the same
  // node around. The closing flag is derived live as `!open`, so the frame `open`
  // flips false the *existing* node gains `.is-closing` and the exit transition
  // animates from its current state — then a timer drops `mounted` to unmount.
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
    const id = window.setTimeout(() => setMounted(false), CLOSE_MS);
    return () => window.clearTimeout(id);
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
    <div className={`${scrimClass}${closing ? " is-closing" : ""}`} onClick={dismiss}>
      <div
        className={`sheet-frame${closing ? " is-closing" : ""}`}
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
