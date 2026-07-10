import { useEffect, useRef } from "react";

import { managerApi } from "../../services/managerApi";
import type { DownloadProgress, OperationKind, OperationSnapshot } from "../../shared/types";
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
  applySnapshotProgress: (progress: DownloadProgress | null | undefined) => void;
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
      setPausedRef.current(null);
      onOperationEndedRef.current();
    };

    const applySnap = (snap: OperationSnapshot) => {
      const busy = busyFromKind(snap.kind);
      if (!busy) return false;
      setBusyRef.current(busy);
      applySnapshotProgressRef.current(snap.progress ?? null);
      if (snap.paused) {
        const kind = pausedKind(snap.kind);
        if (kind) {
          setPausedRef.current({ kind, dl: snap.progress ?? null });
        }
      } else {
        setPausedRef.current(null);
      }
      return true;
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
          if (!next || next.id !== attachedId) {
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
      if (cancelled || !snap) return;
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
