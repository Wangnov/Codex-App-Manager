import { getCurrentWindow } from "@tauri-apps/api/window";

/** Marker attribute for window drag regions (bars, rail, spacers). Only the
 *  element that *directly* receives the mousedown counts, so buttons living
 *  inside a bar stay clickable without opting out. */
export const DRAG_REGION_ATTR = "data-app-drag";

/** Frameless-window dragging + double-click zoom, replacing tauri's built-in
 *  `data-tauri-drag-region` listener. The built-in one calls `start_dragging`
 *  on the FIRST mousedown; on macOS the OS drag session then swallows the
 *  rest of a double-click, so it could restore a maximized window (where the
 *  drag session doesn't engage) but never maximize a normal one. Deferring
 *  `start_dragging` until the pointer actually moves keeps both clicks inside
 *  the webview and makes the double-click toggle deterministic — dragging
 *  still starts on the first pixel of movement.
 */
export function installWindowDragHandler(options: {
  isTauri: () => boolean;
  /** Compact is a fixed-size popover: dragging yes, zooming no. */
  canToggleMaximize: () => boolean;
}): () => void {
  let pressed: { x: number; y: number } | null = null;

  const isDragRegion = (target: EventTarget | null): target is HTMLElement =>
    target instanceof HTMLElement && target.hasAttribute(DRAG_REGION_ATTR);

  const onMouseDown = (event: MouseEvent) => {
    if (event.button !== 0 || !isDragRegion(event.target)) return;
    event.preventDefault(); // no text cursor / selection on the bar
    if (!options.isTauri()) return;
    if (event.detail >= 2) {
      pressed = null;
      if (options.canToggleMaximize()) void getCurrentWindow().toggleMaximize();
      return;
    }
    // Screen coordinates: client coordinates shift under the pointer the
    // moment the window itself moves.
    pressed = { x: event.screenX, y: event.screenY };
  };

  const onMouseMove = (event: MouseEvent) => {
    if (!pressed) return;
    if (Math.abs(event.screenX - pressed.x) + Math.abs(event.screenY - pressed.y) < 2) return;
    pressed = null;
    void getCurrentWindow().startDragging();
  };

  const reset = () => {
    pressed = null;
  };

  document.addEventListener("mousedown", onMouseDown, true);
  document.addEventListener("mousemove", onMouseMove, true);
  document.addEventListener("mouseup", reset, true);
  window.addEventListener("blur", reset);
  return () => {
    document.removeEventListener("mousedown", onMouseDown, true);
    document.removeEventListener("mousemove", onMouseMove, true);
    document.removeEventListener("mouseup", reset, true);
    window.removeEventListener("blur", reset);
  };
}
