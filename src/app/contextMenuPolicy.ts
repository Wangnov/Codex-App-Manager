/**
 * Production WebView context-menu policy.
 *
 * Release builds must not expose browser chrome (Print / Reload / Inspect).
 * Editable controls keep the platform text-editing affordances so copy/cut/paste
 * still work via keyboard and, where the engine shows one, a native edit menu.
 *
 * Dev builds leave the default menu alone so Reload / DevTools stay available.
 */

const EDITABLE_SELECTOR = [
  "input",
  "textarea",
  '[contenteditable=""]',
  '[contenteditable="true"]',
  '[contenteditable="plaintext-only"]',
  '[role="textbox"]',
].join(", ");

function eventTargetElement(target: EventTarget | null): Element | null {
  if (target instanceof Text) return target.parentElement;
  if (target instanceof Element) return target;
  return null;
}

/** True when the event target (or an ancestor) is a text-editing control. */
export function isEditableContextTarget(target: EventTarget | null): boolean {
  const start = eventTargetElement(target);
  if (!start) return false;
  const el = start.closest(EDITABLE_SELECTOR);
  if (!el) {
    // Inherited contentEditable is not always reflected as an attribute match.
    let node: Element | null = start;
    while (node) {
      if (node instanceof HTMLElement && node.isContentEditable) return true;
      node = node.parentElement;
    }
    return false;
  }
  if (el instanceof HTMLInputElement) {
    // Buttons/checkboxes are "input" but not text-editing surfaces.
    const type = (el.type || "text").toLowerCase();
    return ![
      "button",
      "checkbox",
      "radio",
      "submit",
      "reset",
      "file",
      "image",
      "range",
      "color",
      "hidden",
    ].includes(type);
  }
  if (el instanceof HTMLTextAreaElement) return true;
  if (el.getAttribute("role") === "textbox") return true;
  // Prefer the live contentEditable flag; also honor the attribute so jsdom
  // (and similar test envs where isContentEditable stays false) still match.
  if (el instanceof HTMLElement && el.isContentEditable) return true;
  const ce = el.getAttribute("contenteditable");
  return ce === "" || ce === "true" || ce === "plaintext-only";
}

/**
 * Install the production policy. No-op when `enabled` is false (dev builds).
 * Returns a disposer for tests.
 */
export function installContextMenuPolicy(enabled = !import.meta.env.DEV): () => void {
  if (!enabled || typeof document === "undefined") {
    return () => {};
  }

  const onContextMenu = (event: Event) => {
    if (isEditableContextTarget(event.target)) return;
    event.preventDefault();
  };

  document.addEventListener("contextmenu", onContextMenu, true);
  return () => document.removeEventListener("contextmenu", onContextMenu, true);
}
