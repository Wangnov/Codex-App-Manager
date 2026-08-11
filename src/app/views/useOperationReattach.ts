import { useEffect, useRef } from "react";

import { managerApi } from "../../services/managerApi";
import type {
  DownloadProgress,
  OperationKind,
  OperationSnapshot,
} from "../../shared/types";
import type { PausedDownload } from "./ProgressScreen";
import type { StartDlListenOptions } from "./useDownloadProgress";

export type ReattachBusy = "perform" | "install";

function busyFromKind(kind: OperationKind): ReattachBusy | null {
  if (kind === "install") return "install";
  if (kind === "update") return "perform";
  return null;
}

function pausedKind(kind: OperationKind): PausedDownload["kind"] | null {
  if (kind === "install") return "install";
  if (kind === "update") return "perform";
  return null;
}

/**
 * On mount: query the backend operation snapshot, restore progress/busy UI,
 * re-subscribe to the download event channel filtered by operation id, and
 * poll until the lease ends (the original invoke promise is gone after reload).
 */
export function useOperationReattach(opts: {
  startDlListen: (options?: StartDlListenOptions) => Promise<() => void>;
  applySnapshotProgress: (
    progress: DownloadProgress | null | undefined,
  ) => void;
  resetStop: () => void;
  setBusy: (busy: ReattachBusy | null) => void;
  setPaused: (paused: PausedDownload | null) => void;
  /** Called once when the reattached op finishes (success or failure). */
  onOperationEnded: () => void;
  /** True while a local perform/install is already driving the UI. */
  isLocallyBusy: () => boolean;
}) {
  const {
    startDlListen,
    applySnapshotProgress,
    resetStop,
    setBusy,
    setPaused,
    onOperationEnded,
    isLocallyBusy,
  } = opts;

  // Stable refs so the one-shot mount effect does not re-run when callbacks change.
  const startDlListenRef = useRef(startDlListen);
  const applySnapshotProgressRef = useRef(applySnapshotProgress);
  const resetStopRef = useRef(resetStop);
  const setBusyRef = useRef(setBusy);
  const setPausedRef = useRef(setPaused);
  const onOperationEndedRef = useRef(onOperationEnded);
  const isLocallyBusyRef = useRef(isLocallyBusy);
  useEffect(() => {
    startDlListenRef.current = startDlListen;
    applySnapshotProgressRef.current = applySnapshotProgress;
    resetStopRef.current = resetStop;
    setBusyRef.current = setBusy;
    setPausedRef.current = setPaused;
    onOperationEndedRef.current = onOperationEnded;
    isLocallyBusyRef.current = isLocallyBusy;
  });

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    let pollTimer: ReturnType<typeof setTimeout> | null = null;
    let attachedId: string | null = null;
    let lastPaused: PausedDownload | null = null;

    const clearPoll = () => {
      if (pollTimer != null) {
        clearTimeout(pollTimer);
        pollTimer = null;
      }
    };

    const finish = () => {
      clearPoll();
      if (unlisten) {
        unlisten();
        unlisten = null;
      }
      attachedId = null;
      resetStopRef.current();
      setBusyRef.current(null);
      // A pause terminates the native transfer/lease by design, but its cached
      // partial remains resumable. Preserve the last backend snapshot after the
      // lease disappears; clearing it here would either lose resume entirely or
      // make a host fall back to its ordinary latest-version flow.
      if (!lastPaused) setPausedRef.current(null);
      onOperationEndedRef.current();
    };

    const applySnap = (snap: OperationSnapshot) => {
      const busy = snap.resume?.kind ?? busyFromKind(snap.kind);
      if (!busy) return false;
      setBusyRef.current(busy);
      applySnapshotProgressRef.current(snap.progress ?? null);
      if (snap.paused) {
        const kind = snap.resume?.kind ?? pausedKind(snap.kind);
        if (kind) {
          const paused: PausedDownload = { kind, dl: snap.progress ?? null };
          if (snap.historical) paused.historical = snap.historical;
          if (snap.resume) paused.resume = snap.resume;
          if (snap.resume?.installRoot) {
            paused.installRoot = snap.resume.installRoot;
          }
          lastPaused = paused;
          setPausedRef.current(paused);
        }
      } else {
        lastPaused = null;
        setPausedRef.current(null);
      }
      return true;
    };

    const terminalPause = async (expectedId?: string) => {
      const snap = await Promise.resolve()
        .then(() => managerApi.getPausedOperationSnapshot())
        .catch(() => null);
      if (!snap?.paused) return null;
      if (!expectedId || snap.id === expectedId) return snap;

      // A resumed historical download gets a fresh operation id while the
      // backend deliberately retains the original pause id until a terminal
      // outcome is known. Only reconnect that older target when the fresh
      // attempt is proven safe to retry.
      const completion = await Promise.resolve()
        .then(() => managerApi.getOperationCompletion(expectedId))
        .catch(() => null);
      if (completion?.id !== expectedId) return null;
      return completion.state === "failed-before-commit" ||
        completion.state === "rolled-back"
        ? snap
        : null;
    };

    const poll = () => {
      clearPoll();
      pollTimer = setTimeout(() => {
        void (async () => {
          if (cancelled || !attachedId) return;
          const next = await Promise.resolve()
            .then(() => managerApi.getOperationSnapshot())
            .catch(() => null);
          if (cancelled) return;
          if (!next) {
            // Pause aborts the native transfer and releases its lease. The
            // 800ms poll can therefore miss the active `paused=true` window;
            // recover the backend-retained terminal snapshot before finishing.
            const paused = await terminalPause(attachedId);
            if (cancelled) return;
            if (paused) applySnap(paused);
            finish();
            return;
          }
          if (next.id !== attachedId) {
            finish();
            return;
          }
          applySnap(next);
          poll();
        })();
      }, 800);
    };

    void (async () => {
      // Local perform/install already owns the UI — don't double-attach.
      if (isLocallyBusyRef.current()) return;

      const snap = await Promise.resolve()
        .then(() => managerApi.getOperationSnapshot())
        .catch(() => null);
      if (cancelled) return;
      if (!snap) {
        const paused = await terminalPause();
        if (cancelled || !paused || isLocallyBusyRef.current()) return;
        if (applySnap(paused)) finish();
        return;
      }
      if (isLocallyBusyRef.current()) return;

      const busy = busyFromKind(snap.kind);
      if (!busy) return;

      attachedId = snap.id;
      if (!applySnap(snap)) {
        attachedId = null;
        return;
      }

      unlisten = await startDlListenRef.current({
        operationId: snap.id,
        preserveProgress: true,
      });
      if (cancelled) {
        unlisten();
        unlisten = null;
        return;
      }
      poll();
    })();

    return () => {
      cancelled = true;
      clearPoll();
      if (unlisten) unlisten();
    };
  }, []);
}
