/** Cross-cutting "navigation is locked" signal.
 *
 *  The full-screen ProgressScreen deliberately offers no way out while an
 *  install/update runs — every path away from it (start another operation,
 *  trigger the manager's own updater from About, unmount the view holding the
 *  operation's progress events) breaks the OperationManager's serialization
 *  story. The compact layout enforces that by simply having no other controls;
 *  the expanded rail would reintroduce them, so it subscribes here.
 *
 *  Deliberately not React context: holders (ProgressScreen) and consumers
 *  (Rail) sit in unrelated subtrees, and the consumer unmounts/remounts with
 *  the window mode — module state + subscription survives both. */

let holders = 0;
const listeners = new Set<() => void>();

function notify() {
  listeners.forEach((listener) => listener());
}

/** Take the lock; returns the release. Usable directly as a mount effect:
 *  `useEffect(() => acquireNavLock(), [])`. Re-entrant (counted), so React
 *  StrictMode double-mounts and future second holders are safe. */
export function acquireNavLock(): () => void {
  holders += 1;
  if (holders === 1) notify();
  let released = false;
  return () => {
    if (released) return;
    released = true;
    holders -= 1;
    if (holders === 0) notify();
  };
}

export function navLocked(): boolean {
  return holders > 0;
}

/** `useSyncExternalStore`-shaped subscription. */
export function subscribeNavLock(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
