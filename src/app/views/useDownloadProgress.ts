import { useCallback, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";

import type { DownloadProgress } from "../../shared/types";
import { messageFailure, resolveFailure, type FailureSurface } from "../errorCopy";
import { useI18n } from "../i18n";
import { useCountUp } from "../useCountUp";
import type { DownloadStopIntent } from "./ProgressScreen";

export type StartDlListenOptions = {
  /** When reattaching, bind to this operation id and reject foreign events. */
  operationId?: string | null;
  /** Keep the last known progress (reattach) instead of clearing to empty. */
  preserveProgress?: boolean;
};

/** The live-download state machine shared by the Mac and Windows homes: the
 *  progress bytes + eased readouts, the pause/cancel intent, and the backend
 *  stop request. Platform differences are injected — the event channel name and
 *  the pause/cancel commands. Errors surface through `onError` so the host view
 *  can drive its own banner (with optional raw detail disclosure). */
export function useDownloadProgress(opts: {
  eventName: string;
  pauseDownload: () => Promise<boolean>;
  cancelDownload: () => Promise<boolean>;
  cannotCancelMessage: string;
  onError: (failure: FailureSurface | null) => void;
}) {
  const { eventName, pauseDownload, cancelDownload, cannotCancelMessage, onError } = opts;
  const { t } = useI18n();

  const [dl, setDl] = useState<DownloadProgress | null>(null);
  const [speed, setSpeed] = useState(0);
  const dlSample = useRef<{ t: number; bytes: number } | null>(null);
  const [downloadStop, setDownloadStop] = useState<DownloadStopIntent | null>(null);
  const [downloadStopBusy, setDownloadStopBusy] = useState(false);
  const downloadStopRef = useRef<DownloadStopIntent | null>(null);
  // Latest live progress, read at pause time to snapshot the paused figures
  // (the `dl` state is cleared when the perform/install call unwinds).
  const dlRef = useRef<DownloadProgress | null>(null);
  // Active operation id: latched from the first tagged event, or set on reattach.
  // Events carrying a *different* id are dropped so a late packet from op A
  // cannot paint onto op B after a new task started (or after reload).
  const activeOperationIdRef = useRef<string | null>(null);

  // Smoothly roll the live figures instead of snapping on every progress event.
  const dlPctTarget = dl && dl.total > 0 ? Math.min(100, (dl.downloaded / dl.total) * 100) : 0;
  const dlPct = useCountUp(dlPctTarget);
  const dlBytes = useCountUp(dl?.downloaded ?? 0);
  const dlSpeed = useCountUp(speed);

  const onDlProgress = useCallback((event: { payload: DownloadProgress }) => {
    const p = event.payload;
    const eventOpId = typeof p.operationId === "string" && p.operationId ? p.operationId : null;
    const activeId = activeOperationIdRef.current;

    if (eventOpId) {
      if (activeId && eventOpId !== activeId) {
        // Late event for a previous operation — ignore.
        return;
      }
      if (!activeId) {
        // First tagged event for this listen session: latch the id.
        activeOperationIdRef.current = eventOpId;
      }
    } else if (activeId) {
      // We already bound to a concrete op (reattach); untagged events are foreign.
      return;
    }

    setDl(p);
    dlRef.current = p;
    const now = Date.now();
    const prev = dlSample.current;
    if (!prev) {
      dlSample.current = { t: now, bytes: p.downloaded };
    } else if (now > prev.t + 400) {
      setSpeed((p.downloaded - prev.bytes) / ((now - prev.t) / 1000));
      dlSample.current = { t: now, bytes: p.downloaded };
    }
  }, []);

  const startDlListen = useCallback(
    async (options?: StartDlListenOptions) => {
      activeOperationIdRef.current = options?.operationId ?? null;
      if (!options?.preserveProgress) {
        setDl(null);
        dlRef.current = null;
        setSpeed(0);
        dlSample.current = null;
      }
      try {
        return await listen<DownloadProgress>(eventName, onDlProgress);
      } catch {
        // Non-Tauri (web preview): no event bus — nothing to clean up.
        return () => {};
      }
    },
    [eventName, onDlProgress],
  );

  /** Seed progress state from a backend snapshot (reload reattach). */
  const applySnapshotProgress = useCallback((progress: DownloadProgress | null | undefined) => {
    if (!progress) return;
    setDl(progress);
    dlRef.current = progress;
    setSpeed(0);
    dlSample.current = { t: Date.now(), bytes: progress.downloaded };
  }, []);

  const clearActiveOperation = useCallback(() => {
    activeOperationIdRef.current = null;
  }, []);

  const requestDownloadStop = useCallback(
    async (intent: DownloadStopIntent) => {
      onError(null);
      setDownloadStop(intent);
      setDownloadStopBusy(true);
      downloadStopRef.current = intent;
      try {
        const active = intent === "pause" ? await pauseDownload() : await cancelDownload();
        if (!active) {
          downloadStopRef.current = null;
          setDownloadStop(null);
          setDownloadStopBusy(false);
          onError(messageFailure(cannotCancelMessage, "cancelled"));
        }
      } catch (cause) {
        downloadStopRef.current = null;
        setDownloadStop(null);
        setDownloadStopBusy(false);
        onError(resolveFailure(cause, t));
      }
    },
    [pauseDownload, cancelDownload, cannotCancelMessage, onError, t],
  );

  // Clear the transfer + stop state when a perform/install call unwinds.
  const resetStop = useCallback(() => {
    setDl(null);
    setDownloadStop(null);
    setDownloadStopBusy(false);
    downloadStopRef.current = null;
    activeOperationIdRef.current = null;
  }, []);

  return {
    dl,
    setDl,
    dlRef,
    dlPct,
    dlBytes,
    dlSpeed,
    downloadStop,
    downloadStopBusy,
    downloadStopRef,
    activeOperationIdRef,
    startDlListen,
    applySnapshotProgress,
    clearActiveOperation,
    requestDownloadStop,
    resetStop,
  };
}
