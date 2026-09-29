import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";

import {
  errorCode,
  IDLE_MANAGER_UPDATE_SNAPSHOT,
  managerApi,
  SETTINGS_CHANGED_EVENT,
  type ManagerUpdateAvailable,
} from "../services/managerApi";
import type { AppSettings, ManagerUpdateSnapshot } from "../shared/types";
import { mib } from "./format";
import { StatusBanner, Ring } from "./components";
import { userErrorMessage } from "./errorCopy";
import { useI18n } from "./i18n";
import { Sheet } from "./Sheet";

/**
 * Reads the Manager's own self-update progress straight from the Rust
 * backend (`ManagerState.manager_update`) instead of a promise any single
 * view happens to be awaiting. Any view that calls this hook — Home, WinHome,
 * About — sees the exact same phase/bytes for the lifetime of one
 * check-confirm-download-install cycle, including one started from a
 * different window.
 */
export function useManagerUpdateRuntime(): ManagerUpdateSnapshot {
  const [snapshot, setSnapshot] = useState<ManagerUpdateSnapshot>(
    IDLE_MANAGER_UPDATE_SNAPSHOT,
  );

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    // A live event always wins over the one-shot fetch below: once the
    // backend has pushed anything, a fetch that was already in flight can
    // only be describing an earlier moment, so it must not clobber it.
    let receivedLiveEvent = false;

    // Registering the listener FIRST (and only fetching the current snapshot
    // once that registration has actually completed) closes the gap where a
    // phase change between "read the snapshot" and "start listening" would
    // otherwise be missed entirely — most harmful for a terminal
    // installed/error snapshot, which nothing would ever re-emit.
    void managerApi
      .onManagerUpdateRuntime((next) => {
        receivedLiveEvent = true;
        if (!disposed) setSnapshot(next);
      })
      .then((fn) => {
        if (disposed) {
          fn();
          return undefined;
        }
        unlisten = fn;
        return managerApi.getManagerUpdateRuntime();
      })
      .then((initial) => {
        if (!initial || disposed || receivedLiveEvent) return;
        setSnapshot(initial);
      })
      .catch(() => undefined);

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return snapshot;
}

export interface ManagerUpdatePromptController {
  update: ManagerUpdateAvailable | null;
  check: () => Promise<void>;
  refresh: () => Promise<void>;
  setChecksPaused: (paused: boolean) => void;
}

/**
 * Owns startup and periodic checks. Home calls this hook at its stable
 * platform-dispatch layer, so Codex progress/finished screens can hide the
 * prompt without restarting the schedule. Manual home checks share the same
 * in-flight request; returning from settings does not remount the checker.
 */
export function useManagerUpdatePrompt(): ManagerUpdatePromptController {
  const [update, setUpdate] = useState<ManagerUpdateAvailable | null>(null);
  const updateRef = useRef<ManagerUpdateAvailable | null>(null);
  const mountedRef = useRef(false);
  const checkingRef = useRef<Promise<void> | null>(null);
  const checksPausedRef = useRef(false);
  const [settings, setSettings] = useState<AppSettings | null>(null);

  const setChecksPaused = useCallback((paused: boolean) => {
    checksPausedRef.current = paused;
  }, []);

  const replaceUpdate = useCallback((next: ManagerUpdateAvailable | null) => {
    const previous = updateRef.current;
    updateRef.current = next;
    if (mountedRef.current) setUpdate(next);
    if (previous && previous !== next) void previous.discard();
  }, []);

  const check = useCallback((): Promise<void> => {
    if (checksPausedRef.current) return Promise.resolve();
    if (checkingRef.current) return checkingRef.current;

    const pending = managerApi
      .checkManagerUpdate()
      .then((result) => {
        // A background result must not replace/discard the version the user
        // is confirming or installing, even if its request started earlier.
        if (!mountedRef.current || checksPausedRef.current) {
          if (result.kind === "available") void result.discard();
          return;
        }
        if (result.kind === "available") {
          replaceUpdate(result);
          return;
        }
        // An offline/feed failure is not evidence that a known update vanished.
        if (result.kind === "none") replaceUpdate(null);
      })
      .catch(() => {
        // Startup checks are deliberately quiet. About keeps the explicit
        // check path and its localized failure message for troubleshooting.
      })
      .finally(() => {
        if (checkingRef.current === pending) checkingRef.current = null;
      });
    checkingRef.current = pending;
    return pending;
  }, [replaceUpdate]);

  useEffect(() => {
    mountedRef.current = true;
    let active = true;
    let settingsChanged = false;
    const onSettingsChanged = (event: Event) => {
      settingsChanged = true;
      setSettings((event as CustomEvent<AppSettings>).detail);
    };
    window.addEventListener(SETTINGS_CHANGED_EVENT, onSettingsChanged);

    // Fail closed when local settings cannot be read: an uncertain preference
    // must not become an unsolicited network request.
    void managerApi
      .getSettingsStrict()
      .then((settings) => {
        // A slow startup read must not overwrite a newer saved preference.
        if (!active || settingsChanged) return;
        setSettings(settings);
        if (settings.checkOnStartup) void check();
      })
      .catch(() => undefined);

    return () => {
      active = false;
      mountedRef.current = false;
      window.removeEventListener(SETTINGS_CHANGED_EVENT, onSettingsChanged);
      const retained = updateRef.current;
      updateRef.current = null;
      void retained?.discard();
    };
  }, [check]);

  const periodicCheck = settings?.periodicCheck ?? false;
  const intervalSeconds = settings?.periodicCheckIntervalSeconds ?? 900;
  useEffect(() => {
    if (!periodicCheck) return;
    const id = window.setInterval(
      () => void check(),
      Math.max(60_000, intervalSeconds * 1000),
    );
    return () => window.clearInterval(id);
  }, [check, periodicCheck, intervalSeconds]);

  const refresh = useCallback(async () => {
    // A stale expectation can never succeed on retry. Remove it before asking
    // the signed feed for fresh metadata and requiring a new confirmation.
    replaceUpdate(null);
    await check();
  }, [check, replaceUpdate]);

  return useMemo(
    () => ({ update, check, refresh, setChecksPaused }),
    [check, refresh, setChecksPaused, update],
  );
}

/**
 * Reuses the existing home status banner and signed updater confirmation.
 * Routine offline/feed failures stay quiet; the About page remains the manual
 * diagnostics path.
 */
export function ManagerUpdatePrompt({
  update,
  refresh,
  setChecksPaused,
}: ManagerUpdatePromptController) {
  const { t } = useI18n();
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  // Dismissing the banner ("remind me later") is tracked by the exact update
  // object rather than a boolean: `check()` hands out a brand-new object on
  // every successful "available" result (see `replaceUpdate`), so the next
  // periodic/manual check — even one that finds the same version again —
  // naturally un-dismisses the banner instead of hiding it forever.
  const [dismissed, setDismissed] = useState<ManagerUpdateAvailable | null>(
    null,
  );
  const runtime = useManagerUpdateRuntime();
  const [relaunching, setRelaunching] = useState(false);
  const titleId = useId();
  const bodyId = useId();

  // True whenever the backend is actually mid-flight, independent of whether
  // *this* mount is the one that started it — a renderer reload during a
  // self-update loses `installing`/`update` (both local component state) but
  // not the backend's own snapshot, so a fresh mount must still be able to
  // show it and let the user finish (relaunch) or recover (retry).
  const runtimeBusy =
    runtime.phase === "downloading" || runtime.phase === "installing";
  const runtimeDone = runtime.phase === "installed";
  const runtimeFailed = runtime.phase === "error";
  // Keyed on `installing` (this mount's own in-flight install), not on
  // `update` being set: a startup/periodic check can legitimately find an
  // "available" result — e.g. the running process still reports its old
  // version because the just-installed update is awaiting relaunch — while
  // the runtime is still `installed`/`error` from a cycle this mount never
  // locally drove (most commonly a renderer reload mid-update). The runtime
  // must win in that case, or the recovery sheet gets silently replaced by a
  // fresh "update available" banner offering to install the same bits again.
  const reattached = !installing && (runtimeBusy || runtimeDone || runtimeFailed);

  useEffect(() => () => setChecksPaused(false), [setChecksPaused]);

  const closeConfirm = useCallback(() => {
    if (installing || runtimeBusy) return;
    setChecksPaused(false);
    setConfirmOpen(false);
    setFailure(null);
    // A terminal snapshot must not linger and confuse another view (e.g.
    // About) that starts watching the runtime afresh after this one gave up
    // or finished.
    if (runtime.phase === "error" || runtime.phase === "installed") {
      void managerApi.ackManagerUpdateRuntime();
    }
  }, [installing, runtime.phase, runtimeBusy, setChecksPaused]);

  const relaunchNow = useCallback(async () => {
    setRelaunching(true);
    setFailure(null);
    try {
      await managerApi.relaunchManager();
    } catch (cause) {
      // Most commonly a genuine Block (an uninterruptible Codex operation
      // elsewhere) — the backend already released the reservation, so this
      // button stays clickable and the user can just try again once it
      // finishes.
      setFailure(userErrorMessage(cause, t));
    } finally {
      setRelaunching(false);
    }
  }, [t]);

  const retryAfterFailure = useCallback(async () => {
    await managerApi.ackManagerUpdateRuntime();
    await refresh();
  }, [refresh]);

  const installUpdate = useCallback(async () => {
    if (!update || installing) return;
    setInstalling(true);
    setFailure(null);
    try {
      await update.installAndRelaunch();
    } catch (cause) {
      if (errorCode(cause) === "stale_expectation") {
        setChecksPaused(false);
        setConfirmOpen(false);
        await refresh();
      } else {
        setFailure(userErrorMessage(cause, t));
      }
    } finally {
      setInstalling(false);
    }
  }, [installing, refresh, setChecksPaused, t, update]);

  // A reattached recovery sheet always wins over the ordinary "update
  // available" banner — showing both at once would let the user re-confirm
  // installing a version that (per the runtime) is already installed and
  // just awaiting relaunch, or already failed and awaiting retry.
  const showBanner = Boolean(update) && dismissed !== update && !reattached;
  if (!showBanner && !reattached) return null;

  const showProgress =
    (installing || (reattached && runtimeBusy)) &&
    (runtime.phase === "downloading" || runtime.phase === "installing");
  const downloadPct =
    runtime.phase === "downloading" && runtime.total
      ? Math.min(100, Math.round((runtime.downloaded / runtime.total) * 100))
      : null;

  return (
    <>
      {showBanner && update ? (
        <div className="manager-update-prompt">
          <StatusBanner
            tone="info"
            icon="arrowUp"
            action={
              <button
                type="button"
                className="btn primary sm"
                onClick={() => {
                  setChecksPaused(true);
                  setFailure(null);
                  setConfirmOpen(true);
                }}
                disabled={installing}
              >
                {t("confirm.ok")}
              </button>
            }
            onClose={() => setDismissed(update)}
          >
            {t("about.mgrFound", { version: update.version })}
          </StatusBanner>
        </div>
      ) : null}

      <Sheet
        open={confirmOpen || reattached}
        onDismiss={closeConfirm}
        dismissable={!installing && !runtimeBusy}
        labelledBy={titleId}
        describedBy={bodyId}
        initialFocus="dismiss"
      >
        <Ring icon="arrowUp" />
        {/* `reattached` is checked first throughout this sheet: it can be
            true even while `update` is also set (a stale startup check found
            an "available" result while the runtime is still installed/error
            from a cycle this mount never locally drove), and the runtime's
            terminal state must win so the recovery action stays reachable. */}
        <h3 id={titleId}>
          {reattached
            ? runtime.version
              ? t("confirm.title", { version: runtime.version })
              : t("progress.title")
            : update
              ? t("confirm.title", { version: update.version })
              : ""}
        </h3>
        {reattached ? (
          runtimeBusy ? (
            <p id={bodyId}>{t("about.mgrConfirmBody")}</p>
          ) : runtimeDone ? (
            <p id={bodyId}>{t("progress.updateInstalled")}</p>
          ) : (
            <p id={bodyId}>{t("about.mgrUnavailable")}</p>
          )
        ) : update ? (
          <p id={bodyId}>{t("about.mgrConfirmBody")}</p>
        ) : null}
        {showProgress ? (
          <div className="mgr-update-progress" aria-live="polite">
            <div className="sub">
              {runtime.phase === "installing"
                ? t("progress.installing")
                : t("progress.title")}
            </div>
            <div
              className="bar"
              role="progressbar"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={downloadPct ?? undefined}
            >
              <div
                className={`bar-fill${downloadPct === null ? " indeterminate" : ""}`}
                style={downloadPct === null ? undefined : { width: `${downloadPct}%` }}
              />
            </div>
            {runtime.phase === "downloading" && runtime.total ? (
              <div className="dlmeta">
                {mib(runtime.downloaded)} / {mib(runtime.total)}
              </div>
            ) : null}
          </div>
        ) : null}
        {reattached && runtimeFailed && runtime.message ? (
          <StatusBanner tone="err">{runtime.message}</StatusBanner>
        ) : failure ? (
          <StatusBanner tone="err">{failure}</StatusBanner>
        ) : null}
        <div className="row2 sheet-actions">
          {reattached ? (
            runtimeBusy ? null : runtimeDone ? (
              <>
                <button type="button" className="btn ghost" onClick={closeConfirm}>
                  {t("confirm.cancel")}
                </button>
                <button
                  type="button"
                  className="btn primary"
                  onClick={() => void relaunchNow()}
                  disabled={relaunching}
                >
                  {t("progress.relaunchNow")}
                </button>
              </>
            ) : (
              <>
                <button type="button" className="btn ghost" onClick={closeConfirm}>
                  {t("confirm.cancel")}
                </button>
                <button
                  type="button"
                  className="btn primary"
                  onClick={() => void retryAfterFailure()}
                >
                  {t("settings.retry")}
                </button>
              </>
            )
          ) : (
            <>
              <button
                type="button"
                className="btn ghost"
                onClick={closeConfirm}
                disabled={installing}
              >
                {t("confirm.cancel")}
              </button>
              <button
                type="button"
                className="btn primary"
                onClick={() => void installUpdate()}
                disabled={installing}
              >
                {installing
                  ? runtime.phase === "installing"
                    ? t("progress.installing")
                    : t("progress.title")
                  : t("confirm.ok")}
              </button>
            </>
          )}
        </div>
      </Sheet>
    </>
  );
}
