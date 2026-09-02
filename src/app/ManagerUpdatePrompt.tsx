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
  type ManagerUpdateAvailable,
} from "../services/managerApi";
import { StatusBanner, Ring } from "./components";
import { userErrorMessage } from "./errorCopy";
import { useI18n } from "./i18n";
import { Sheet } from "./Sheet";

export interface ManagerUpdatePromptController {
  update: ManagerUpdateAvailable | null;
  refresh: () => Promise<void>;
}

/**
 * Owns the one-per-session startup check. Home calls this hook at its stable
 * platform-dispatch layer, so Codex progress/finished screens can hide the
 * prompt without remounting the checker and contacting the feed again.
 */
export function useManagerUpdatePrompt(): ManagerUpdatePromptController {
  const [update, setUpdate] = useState<ManagerUpdateAvailable | null>(null);
  const updateRef = useRef<ManagerUpdateAvailable | null>(null);
  const mountedRef = useRef(false);
  const checkingRef = useRef<Promise<void> | null>(null);

  const replaceUpdate = useCallback((next: ManagerUpdateAvailable | null) => {
    const previous = updateRef.current;
    updateRef.current = next;
    if (mountedRef.current) setUpdate(next);
    if (previous && previous !== next) void previous.discard();
  }, []);

  const check = useCallback((): Promise<void> => {
    if (checkingRef.current) return checkingRef.current;

    const pending = managerApi
      .checkManagerUpdate()
      .then((result) => {
        if (result.kind === "available") {
          if (mountedRef.current) replaceUpdate(result);
          else void result.discard();
          return;
        }
        if (mountedRef.current) replaceUpdate(null);
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

    // Fail closed when local settings cannot be read: an uncertain preference
    // must not become an unsolicited network request.
    void managerApi
      .getSettingsStrict()
      .then((settings) => {
        if (active && settings.checkOnStartup) void check();
      })
      .catch(() => undefined);

    return () => {
      active = false;
      mountedRef.current = false;
      const retained = updateRef.current;
      updateRef.current = null;
      void retained?.discard();
    };
  }, [check]);

  const refresh = useCallback(async () => {
    // A stale expectation can never succeed on retry. Remove it before asking
    // the signed feed for fresh metadata and requiring a new confirmation.
    replaceUpdate(null);
    await check();
  }, [check, replaceUpdate]);

  return useMemo(() => ({ update, refresh }), [refresh, update]);
}

/**
 * Reuses the existing home status banner and signed updater confirmation.
 * Routine offline/feed failures stay quiet; the About page remains the manual
 * diagnostics path.
 */
export function ManagerUpdatePrompt({
  update,
  refresh,
}: ManagerUpdatePromptController) {
  const { t } = useI18n();
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const titleId = useId();
  const bodyId = useId();

  const closeConfirm = useCallback(() => {
    if (installing) return;
    setConfirmOpen(false);
    setFailure(null);
  }, [installing]);

  const installUpdate = useCallback(async () => {
    if (!update || installing) return;
    setInstalling(true);
    setFailure(null);
    try {
      await update.installAndRelaunch();
    } catch (cause) {
      if (errorCode(cause) === "stale_expectation") {
        setConfirmOpen(false);
        await refresh();
      } else {
        setFailure(userErrorMessage(cause, t));
      }
    } finally {
      setInstalling(false);
    }
  }, [installing, refresh, t, update]);

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
