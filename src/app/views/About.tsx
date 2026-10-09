import { useCallback, useEffect, useId, useState } from "react";

import {
  errorCode,
  managerApi,
  type ManagerUpdateAvailable,
} from "../../services/managerApi";
import { mib } from "../format";
import { useManagerUpdateRuntime, useRelaunchGrace } from "../ManagerUpdatePrompt";
import { codeErrorMessage, userErrorMessage } from "../errorCopy";
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
  const holdUntilExit = useRelaunchGrace();
  // Separate from `mgrMsg`: that one is rendered in the (inert-while-the-
  // sheet-is-open) background row, so a relaunch failure needs its own state
  // shown inside the open sheet, next to the button the user just clicked.
  const [relaunchFailure, setRelaunchFailure] = useState<string | null>(null);
  // Same backend-owned snapshot the Home banner reads: if a self-update was
  // started from Home and the user then opens About, both show the exact
  // same download/install progress instead of About guessing from nothing.
  const runtime = useManagerUpdateRuntime();
  // Cancelling the "installed, awaiting relaunch" recovery sheet must not
  // lose the only path back to relaunching it — tracked by the exact
  // snapshot's `updatedAtMs` (not a boolean) so a *later* self-update cycle
  // reaching `installed` again naturally un-snoozes instead of staying
  // hidden forever. Mirrors the same fix in `ManagerUpdatePrompt`.
  const [installedSnoozedAt, setInstalledSnoozedAt] = useState<number | null>(
    null,
  );
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
  // The installed bits are already on disk, just awaiting relaunch — acking
  // the runtime on a plain Cancel would erase the only path back to that
  // relaunch action, and the running process still reports its old version,
  // so a later check could then offer to install the exact same bits again.
  // Snooze the *sheet* instead: the persistent reminder banner below keeps
  // the relaunch action reachable without touching the runtime.
  const installedSnoozed =
    runtimeDone && installedSnoozedAt === runtime.updatedAtMs;
  // While the installed sheet is snoozed the runtime no longer owns the
  // modal: a newer version found by a later check must get its own confirm
  // content rather than the stale installed-update sheet.
  // Keyed on `mgrBusy` (this view's own in-flight check/install), not on
  // `pendingUpdate` being set: a manual "check for update" here can find an
  // "available" result — e.g. this process still reports its old version
  // because a just-installed update elsewhere is awaiting relaunch — while
  // the runtime is still `installed`/`error` from a cycle this view never
  // drove itself. The runtime must win, or the recovery action (relaunch/
  // retry) gets silently replaced by a confirm dialog for the same bits.
  const reattached =
    !mgrBusy &&
    !installedSnoozed &&
    (runtimeBusy || runtimeDone || runtimeFailed);
  const showReattachedSheet = reattached;
  const updateSheetOpen = Boolean(pendingUpdate) || showReattachedSheet;

  // The runtime is the single source of truth for "this version is already on
  // disk": drop this view's cached availability result for it whichever view
  // drove the install, so a stale confirm for an installed version can never
  // be offered again (Home does the same for its own cached result). Not while
  // this view is itself mid-install: its own sheet still reads `pendingUpdate`.
  useEffect(() => {
    if (mgrBusy) return;
    if (
      runtime.phase === "installed" &&
      pendingUpdate &&
      pendingUpdate.version === runtime.version
    ) {
      void pendingUpdate.discard();
      setPendingUpdate(null);
      setMgrMsg(null);
    }
  }, [mgrBusy, pendingUpdate, runtime.phase, runtime.version]);

  const closeUpdateConfirm = useCallback(() => {
    if (mgrBusy || runtimeBusy) return;
    void pendingUpdate?.discard();
    setPendingUpdate(null);
    setRelaunchFailure(null);
    if (runtime.phase === "error") {
      // A terminal error must not linger and confuse another view (e.g.
      // Home) that starts watching the runtime afresh after this one gave
      // up.
      void managerApi.ackManagerUpdateRuntime();
    } else if (runtime.phase === "installed") {
      setInstalledSnoozedAt(runtime.updatedAtMs);
    }
  }, [mgrBusy, pendingUpdate, runtime.phase, runtime.updatedAtMs, runtimeBusy]);

  // An explicit discard of the persistent "installed" reminder (its own
  // close button, not the sheet's Cancel) really does mean "I don't want
  // this any more" — ack the runtime for real.
  const dismissInstalledReminder = useCallback(() => {
    setInstalledSnoozedAt(null);
    void managerApi.ackManagerUpdateRuntime();
  }, []);

  const relaunchNow = useCallback(async () => {
    setRelaunching(true);
    setRelaunchFailure(null);
    let accepted = false;
    try {
      await managerApi.relaunchManager();
      accepted = true;
    } catch (cause) {
      // Most commonly a genuine Block (an uninterruptible Codex operation
      // elsewhere) — the backend already released the reservation, so this
      // button stays clickable and the user can just try again once it
      // finishes. Shown inside the sheet (not `mgrMsg`, which sits in the
      // background row that `inert` disables while this sheet is open).
      setRelaunchFailure(userErrorMessage(cause, t));
    } finally {
      // Accepted: the process is about to exit, keep the button busy.
      if (accepted) holdUntilExit(() => setRelaunching(false));
      else setRelaunching(false);
    }
  }, [holdUntilExit, t]);

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
    let relaunchAccepted = false;
    try {
      await pendingUpdate.installAndRelaunch();
      relaunchAccepted = true;
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
      // Install succeeded and the relaunch was accepted: the process is about
      // to exit, so stay busy (not the recovery sheet) until then. A refused
      // relaunch falls through to the recovery UI.
      if (relaunchAccepted) holdUntilExit(() => setMgrBusy(false));
      else setMgrBusy(false);
    }
  }, [checkManager, holdUntilExit, pendingUpdate, t]);

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
      await managerApi.writeClipboardText(formatDiagnostics(diagnostics));
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

        {installedSnoozed ? (
          <div className="manager-update-prompt">
            <StatusBanner
              tone="info"
              icon="arrowUp"
              action={
                <button
                  type="button"
                  className="btn primary sm"
                  onClick={() => void relaunchNow()}
                  disabled={relaunching}
                >
                  {t("progress.relaunchNow")}
                </button>
              }
              onClose={dismissInstalledReminder}
            >
              {t("progress.updateInstalled")}
            </StatusBanner>
            {relaunchFailure ? (
              <StatusBanner tone="err">{relaunchFailure}</StatusBanner>
            ) : null}
          </div>
        ) : null}

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
        {/* `reattached` is checked first, exactly like the Home prompt: it
            can be true even while `pendingUpdate` is also set (a manual
            check found an "available" result while the runtime is still
            installed/error from a cycle this view never drove itself), and
            the runtime's terminal state must win so the recovery action
            stays reachable. */}
        <h3 id={updateTitleId}>
          {reattached
            ? runtime.version
              ? t("confirm.title", { version: runtime.version })
              : t("progress.title")
            : pendingUpdate
              ? t("confirm.title", { version: pendingUpdate.version })
              : ""}
        </h3>
        {reattached ? (
          runtimeBusy ? (
            <p id={updateBodyId}>{t("about.mgrConfirmBody")}</p>
          ) : runtimeDone ? (
            <p id={updateBodyId}>{t("progress.updateInstalled")}</p>
          ) : (
            <p id={updateBodyId}>{t("progress.updateFailed")}</p>
          )
        ) : pendingUpdate ? (
          <p id={updateBodyId}>{t("about.mgrConfirmBody")}</p>
        ) : null}
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
        {reattached && runtimeFailed ? (
          <StatusBanner tone="err">
            {codeErrorMessage(runtime.code, t)}
          </StatusBanner>
        ) : reattached && runtimeDone && relaunchFailure ? (
          <StatusBanner tone="err">{relaunchFailure}</StatusBanner>
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
                {relaunching
                  ? t("progress.relaunching")
                  : t("progress.relaunchNow")}
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
              {mgrBusy
                ? runtime.phase === "installed"
                  ? t("progress.relaunching")
                  : t("progress.installing")
                : t("confirm.ok")}
            </button>
          </div>
        )}
      </Sheet>
    </div>
  );
}
