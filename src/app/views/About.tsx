import { useCallback, useId, useState } from "react";

import {
  errorCode,
  managerApi,
  type ManagerUpdateAvailable,
} from "../../services/managerApi";
import { mib } from "../format";
import { useManagerUpdateRuntime } from "../ManagerUpdatePrompt";
import { userErrorMessage } from "../errorCopy";
import { Icon, CodexMark } from "../icons";
import { useI18n } from "../i18n";
import { NavBar, Ring, StatusBanner } from "../components";
import { formatDiagnostics } from "../diagnostics";
import { Sheet } from "../Sheet";

const APP_VERSION = import.meta.env.VITE_APP_VERSION ?? "0.0.0";
const REPO_URL = "https://github.com/Wangnov/Codex-App-Manager";

export function About({ onBack }: { onBack: () => void }) {
  const { t } = useI18n();
  const [mgrBusy, setMgrBusy] = useState(false);
  const [mgrMsg, setMgrMsg] = useState<string | null>(null);
  const [pendingUpdate, setPendingUpdate] = useState<ManagerUpdateAvailable | null>(null);
  const [relaunching, setRelaunching] = useState(false);
  // Same backend-owned snapshot the Home banner reads: if a self-update was
  // started from Home and the user then opens About, both show the exact
  // same download/install progress instead of About guessing from nothing.
  const runtime = useManagerUpdateRuntime();
  const updateTitleId = useId();
  const updateBodyId = useId();

  // True while the backend is actually downloading/installing, independent
  // of whether *this* view is the one that started it — a check begun from
  // Home leaves `pendingUpdate`/`mgrBusy` here at their initial values, but
  // the runtime snapshot is shared, so About must still reflect it.
  const runtimeBusy =
    runtime.phase === "downloading" || runtime.phase === "installing";
  const runtimeDone = runtime.phase === "installed";
  const runtimeFailed = runtime.phase === "error";
  // Only a *reattached* done/failed snapshot (no local `pendingUpdate`) needs
  // its own sheet + actions; the normal confirm flow below already surfaces
  // its own failure/installing copy while `pendingUpdate` is set.
  const reattached = !pendingUpdate && (runtimeBusy || runtimeDone || runtimeFailed);
  const updateSheetOpen = Boolean(pendingUpdate) || reattached;

  const closeUpdateConfirm = useCallback(() => {
    if (mgrBusy || runtimeBusy) return;
    void pendingUpdate?.discard();
    setPendingUpdate(null);
    if (runtime.phase === "error" || runtime.phase === "installed") {
      void managerApi.ackManagerUpdateRuntime();
    }
  }, [mgrBusy, pendingUpdate, runtime.phase, runtimeBusy]);

  const relaunchNow = useCallback(async () => {
    setRelaunching(true);
    try {
      await managerApi.relaunchManager();
    } catch (cause) {
      // Most commonly a genuine Block (an uninterruptible Codex operation
      // elsewhere) — the backend already released the reservation, so this
      // button stays clickable and the user can just try again once it
      // finishes.
      setMgrMsg(userErrorMessage(cause, t));
    } finally {
      setRelaunching(false);
    }
  }, [t]);

  const checkManager = useCallback(async () => {
    setMgrBusy(true);
    setMgrMsg(null);
    if (pendingUpdate) {
      void pendingUpdate.discard();
      setPendingUpdate(null);
    }
    try {
      const result = await managerApi.checkManagerUpdate();
      if (result.kind === "available") {
        setPendingUpdate(result);
        setMgrMsg(t("about.mgrFound", { version: result.version }));
      } else if (result.kind === "none") {
        setMgrMsg(t("about.mgrUpToDate"));
      } else if (result.kind === "development") {
        setMgrMsg(t("about.mgrDev"));
      } else {
        setMgrMsg(t("about.mgrUnavailable"));
      }
    } catch (cause) {
      setMgrMsg(userErrorMessage(cause, t));
    } finally {
      setMgrBusy(false);
    }
  }, [pendingUpdate, t]);

  // Recovers a *reattached* failed snapshot (no local `pendingUpdate`, e.g.
  // the failure happened while this view wasn't the one driving the install):
  // clear the terminal error back to idle, then run an ordinary check so a
  // fresh confirmation is required, matching the `stale_expectation` path.
  const retryAfterFailure = useCallback(async () => {
    await managerApi.ackManagerUpdateRuntime();
    await checkManager();
  }, [checkManager]);

  const installManagerUpdate = useCallback(async () => {
    if (!pendingUpdate) return;
    setMgrBusy(true);
    setMgrMsg(t("progress.installing"));
    try {
      await pendingUpdate.installAndRelaunch();
    } catch (cause) {
      if (errorCode(cause) === "stale_expectation") {
        // The feed changed after confirmation. Re-read it now so the localized
        // "rechecked" contract is true and any replacement version requires a
        // fresh confirmation.
        await checkManager();
      } else {
        setMgrMsg(userErrorMessage(cause, t));
        setPendingUpdate(null);
      }
    } finally {
      setMgrBusy(false);
    }
  }, [checkManager, pendingUpdate, t]);

  const openLogsDir = useCallback(async () => {
    setMgrMsg(null);
    try {
      await managerApi.openLogsDir();
    } catch (cause) {
      setMgrMsg(userErrorMessage(cause, t));
    }
  }, [t]);

  const copyDiagnostics = useCallback(async () => {
    setMgrMsg(null);
    try {
      const diagnostics = await managerApi.getDiagnostics();
      await navigator.clipboard.writeText(formatDiagnostics(diagnostics));
      setMgrMsg(t("about.diagnosticsCopied"));
    } catch {
      setMgrMsg(t("about.diagnosticsFailed"));
    }
  }, [t]);

  return (
    <div className="pop">
      {/* Block leaving while a self-update is downloading/installing — it
          relaunches the manager process and could interrupt a Codex op started
          back on the home screen. */}
      <NavBar
        title={t("settings.more.about")}
        onBack={onBack}
        disableBack={mgrBusy || runtimeBusy}
      />
      <div className="scroll view" inert={updateSheetOpen ? true : undefined}>
        <section className="hero" style={{ paddingTop: 8 }}>
          <div className="mark mark-lg" style={{ marginBottom: 14 }}>
            <CodexMark />
          </div>
          <div className="headline" style={{ fontSize: 18 }}>
            {t("app.name")}
          </div>
          <div className="sub">{t("about.version", { v: APP_VERSION })}</div>
          <div className="desc">{t("about.tagline")}</div>
        </section>

        <div className="list">
          <button className="row" onClick={checkManager} disabled={mgrBusy}>
            <Icon name="refresh" className="ricon" />
            <span className="rtext">
              <span className="rtitle">{t("about.checkManager")}</span>
              {mgrMsg ? <span className="rsub">{mgrMsg}</span> : null}
            </span>
            <span className="rval">{mgrBusy ? t("about.mgrChecking") : ""}</span>
          </button>
          <button className="row" onClick={() => void managerApi.openUrl(REPO_URL)}>
            <Icon name="message" className="ricon" />
            <span className="rtext">
              <span className="rtitle">{t("about.feedback")}</span>
              <span className="rsub">{REPO_URL.replace("https://", "")}</span>
            </span>
            <Icon name="external" className="chev" />
          </button>
          <button className="row" onClick={openLogsDir}>
            <Icon name="folder" className="ricon" />
            <span className="rtext">
              <span className="rtitle">{t("about.openLogsDir")}</span>
            </span>
            <Icon name="chevron" className="chev" />
          </button>
          <button className="row" onClick={copyDiagnostics}>
            <Icon name="copy" className="ricon" />
            <span className="rtext">
              <span className="rtitle">{t("about.copyDiagnostics")}</span>
            </span>
            <Icon name="chevron" className="chev" />
          </button>
        </div>
      </div>
      <Sheet
        open={updateSheetOpen}
        onDismiss={closeUpdateConfirm}
        dismissable={!mgrBusy && !runtimeBusy}
        labelledBy={updateTitleId}
        describedBy={updateBodyId}
        initialFocus="dismiss"
      >
        <Ring icon="arrowUp" />
        <h3 id={updateTitleId}>
          {pendingUpdate
            ? t("confirm.title", { version: pendingUpdate.version })
            : reattached && runtime.version
              ? t("confirm.title", { version: runtime.version })
              : reattached
                ? t("progress.title")
                : ""}
        </h3>
        {!reattached || runtimeBusy ? (
          <p id={updateBodyId}>{t("about.mgrConfirmBody")}</p>
        ) : runtimeDone ? (
          <p id={updateBodyId}>{t("progress.updateInstalled")}</p>
        ) : (
          <p id={updateBodyId}>{t("about.mgrUnavailable")}</p>
        )}
        {runtimeBusy ? (
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
              aria-valuenow={
                runtime.phase === "downloading" && runtime.total
                  ? Math.min(
                      100,
                      Math.round((runtime.downloaded / runtime.total) * 100),
                    )
                  : undefined
              }
            >
              <div
                className={`bar-fill${
                  runtime.phase === "downloading" && runtime.total
                    ? ""
                    : " indeterminate"
                }`}
                style={
                  runtime.phase === "downloading" && runtime.total
                    ? {
                        width: `${Math.min(100, (runtime.downloaded / runtime.total) * 100)}%`,
                      }
                    : undefined
                }
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
        ) : null}
        {reattached ? (
          runtimeBusy ? null : runtimeDone ? (
            <div className="row2 sheet-actions">
              <button className="btn ghost" onClick={closeUpdateConfirm}>
                {t("confirm.cancel")}
              </button>
              <button
                className="btn primary"
                onClick={() => void relaunchNow()}
                disabled={relaunching}
              >
                {t("progress.relaunchNow")}
              </button>
            </div>
          ) : (
            <div className="row2 sheet-actions">
              <button className="btn ghost" onClick={closeUpdateConfirm} disabled={mgrBusy}>
                {t("confirm.cancel")}
              </button>
              <button
                className="btn primary"
                onClick={() => void retryAfterFailure()}
                disabled={mgrBusy}
              >
                {t("settings.retry")}
              </button>
            </div>
          )
        ) : (
          <div className="row2 sheet-actions">
            <button className="btn ghost" onClick={closeUpdateConfirm} disabled={mgrBusy}>
              {t("confirm.cancel")}
            </button>
            <button className="btn primary" onClick={installManagerUpdate} disabled={mgrBusy}>
              {mgrBusy ? t("progress.installing") : t("confirm.ok")}
            </button>
          </div>
        )}
      </Sheet>
    </div>
  );
}
