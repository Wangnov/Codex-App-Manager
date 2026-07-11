import { useCallback, useState } from "react";

import { managerApi } from "../../services/managerApi";
import { userErrorMessage } from "../errorCopy";
import { Icon, CodexMark } from "../icons";
import { useI18n } from "../i18n";
import { NavBar } from "../components";
import { formatDiagnostics } from "../diagnostics";
import { useManagerUpdate } from "../managerUpdate";

const APP_VERSION = import.meta.env.VITE_APP_VERSION ?? "0.0.0";
const REPO_URL = "https://github.com/Wangnov/Codex-App-Manager";

export function About({ onBack }: { onBack: () => void }) {
  const { t } = useI18n();
  const managerUpdate = useManagerUpdate();
  const [actionMsg, setActionMsg] = useState<string | null>(null);
  const mgrBusy = ["checking", "downloading", "installing", "relaunching"].includes(
    managerUpdate.status,
  );
  const mgrNavigationLocked = ["downloading", "installing", "relaunching"].includes(
    managerUpdate.status,
  );
  let managerStatusMessage: string | null = null;
  if (managerUpdate.failure) {
    managerStatusMessage = managerUpdate.failure.message;
  } else if (managerUpdate.status === "checking") {
    managerStatusMessage = t("about.mgrChecking");
  } else if (managerUpdate.status === "downloading") {
    managerStatusMessage = t("managerUpdate.downloading");
  } else if (managerUpdate.status === "installing") {
    managerStatusMessage = t("progress.installing");
  } else if (managerUpdate.status === "installed-awaiting-relaunch" && managerUpdate.update) {
    managerStatusMessage = t("managerUpdate.restartRequired", {
      version: managerUpdate.update.version,
    });
  } else if (managerUpdate.update) {
    managerStatusMessage = t("about.mgrFound", { version: managerUpdate.update.version });
  } else if (managerUpdate.status === "up-to-date") {
    managerStatusMessage = t("about.mgrUpToDate");
  } else if (managerUpdate.status === "development") {
    managerStatusMessage = t("about.mgrDev");
  }
  let managerStatusValue = "";
  if (managerUpdate.status === "downloading") {
    managerStatusValue = t("managerUpdate.downloading");
  } else if (managerUpdate.status === "installed-awaiting-relaunch") {
    managerStatusValue = t("managerUpdate.restart");
  } else if (mgrBusy) {
    managerStatusValue = t("about.mgrChecking");
  }

  const checkManager = useCallback(async () => {
    setActionMsg(null);
    if (managerUpdate.status === "installed-awaiting-relaunch") {
      managerUpdate.openDetails();
      return;
    }
    await managerUpdate.check({ manual: true, openWhenAvailable: true });
  }, [managerUpdate]);

  const openLogsDir = useCallback(async () => {
    setActionMsg(null);
    try {
      await managerApi.openLogsDir();
    } catch (cause) {
      setActionMsg(userErrorMessage(cause, t));
    }
  }, [t]);

  const copyDiagnostics = useCallback(async () => {
    setActionMsg(null);
    try {
      const diagnostics = await managerApi.getDiagnostics();
      await navigator.clipboard.writeText(formatDiagnostics(diagnostics));
      setActionMsg(t("about.diagnosticsCopied"));
    } catch {
      setActionMsg(t("about.diagnosticsFailed"));
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
        disableBack={mgrNavigationLocked}
      />
      <div className="scroll view">
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
              {managerStatusMessage ? <span className="rsub">{managerStatusMessage}</span> : null}
            </span>
            <span className="rval">{managerStatusValue}</span>
          </button>
          {actionMsg ? <div className="rsub about-action-message">{actionMsg}</div> : null}
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
    </div>
  );
}
