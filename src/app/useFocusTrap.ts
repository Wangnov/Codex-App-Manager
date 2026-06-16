import { useEffect, type RefObject } from "react";

const FOCUSABLE =
  'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';

export function getFocusable(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter((el) => {
    if (el.hasAttribute("hidden") || el.getAttribute("aria-hidden") === "true") {
      return false;
    }
    const style = window.getComputedStyle?.(el);
    return (
      el.offsetParent !== null ||
      el === document.activeElement ||
      !style ||
      (style.display !== "none" && style.visibility !== "hidden")
    );
  });
}

export function cycleFocusTarget(
  items: HTMLElement[],
  active: Element | null,
  shift: boolean,
): HTMLElement | null {
  if (items.length === 0) {
    return null;
  }
  const first = items[0];
  const last = items[items.length - 1];
  if (shift && active === first) {
    return last;
  }
  if (!shift && active === last) {
    return first;
  }
  return null;
}

export function useFocusTrap(
  ref: RefObject<HTMLElement | null>,
  opts: {
    onEsc?: () => void;
    initialFocus?: "first" | "primary" | "dismiss" | "container";
  },
) {
  useEffect(() => {
    const node = ref.current;
    if (!node) {
      return;
    }
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;

    const focusables = getFocusable(node);
    const pick = () => {
      if (opts.initialFocus === "dismiss") {
        return node.querySelector<HTMLElement>(".btn.ghost") ?? focusables[0];
      }
      if (opts.initialFocus === "primary") {
        return node.querySelector<HTMLElement>(".btn.primary, .btn.danger") ?? focusables[0];
      }
      if (opts.initialFocus === "container") {
        node.tabIndex = -1;
        return node;
      }
      return node.querySelector<HTMLElement>('[aria-selected="true"]') ?? focusables[0];
    };
    pick()?.focus();

    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        opts.onEsc?.();
        return;
      }
      if (event.key !== "Tab") {
        return;
      }
      const target = cycleFocusTarget(getFocusable(node), document.activeElement, event.shiftKey);
      if (target) {
        event.preventDefault();
        target.focus();
      }
    };

    node.addEventListener("keydown", onKey);
    return () => {
      node.removeEventListener("keydown", onKey);
      if (opener && document.contains(opener)) {
        opener.focus();
      }
    };
  }, [ref, opts.onEsc, opts.initialFocus]);
}
