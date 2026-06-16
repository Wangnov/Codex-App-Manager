import { useRef, type ReactNode } from "react";

import { useFocusTrap } from "./useFocusTrap";

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
  const dismiss = () => {
    if (dismissable) {
      onDismiss?.();
    }
  };
  useFocusTrap(sheetRef, { onEsc: dismiss, initialFocus });

  if (!open) {
    return null;
  }

  return (
    <div className={scrimClass} onClick={dismiss}>
      <div
        ref={sheetRef}
        className="sheet"
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
        onClick={(event) => event.stopPropagation()}
      >
        {children}
      </div>
    </div>
  );
}
