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
  managerApi,
  SETTINGS_CHANGED_EVENT,
  type ManagerUpdateAvailable,
} from "../services/managerApi";
import type { AppSettings } from "../shared/types";
import { StatusBanner, Ring } from "./components";
import { userErrorMessage } from "./errorCopy";
import { useI18n } from "./i18n";
import { Sheet } from "./Sheet";

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
  const titleId = useId();
  const bodyId = useId();

  useEffect(() => () => setChecksPaused(false), [setChecksPaused]);

  const closeConfirm = useCallback(() => {
    if (installing) return;
    setChecksPaused(false);
    setConfirmOpen(false);
    setFailure(null);
  }, [installing, setChecksPaused]);

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

  if (!update) return null;

  return (
    <>
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
        >
          {t("about.mgrFound", { version: update.version })}
        </StatusBanner>
      </div>

      <Sheet
        open={confirmOpen}
        onDismiss={closeConfirm}
        dismissable={!installing}
        labelledBy={titleId}
        describedBy={bodyId}
        initialFocus="dismiss"
      >
        <Ring icon="arrowUp" />
        <h3 id={titleId}>{t("confirm.title", { version: update.version })}</h3>
        <p id={bodyId}>{t("about.mgrConfirmBody")}</p>
        {failure ? <StatusBanner tone="err">{failure}</StatusBanner> : null}
        <div className="row2 sheet-actions">
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
            {installing ? t("progress.installing") : t("confirm.ok")}
          </button>
        </div>
      </Sheet>
    </>
  );
}
