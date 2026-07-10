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
  "select",
  '[contenteditable=""]',
  '[contenteditable="true"]',
  '[role="textbox"]',
].join(", ");

/** True when the event target (or an ancestor) is a text-editing control. */
export function isEditableContextTarget(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  const el = target.closest(EDITABLE_SELECTOR);
  if (!el) return false;
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
  return true;
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
