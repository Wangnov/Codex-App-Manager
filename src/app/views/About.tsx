import { useCallback, useState } from "react";

import { errorMessage, managerApi, type ManagerUpdateAvailable } from "../../services/managerApi";
import type { Diagnostics } from "../../shared/types";
import { Icon, CodexMark } from "../icons";
import { useI18n } from "../i18n";
import { NavBar, Ring } from "../components";

const APP_VERSION = import.meta.env.VITE_APP_VERSION ?? "0.0.0";
const REPO_URL = "https://github.com/Wangnov/Codex-App-Manager";

function formatDiagnostics(diagnostics: Diagnostics): string {
  const health = diagnostics.configHealth;
  const recentErrors = diagnostics.recentErrors.length
    ? diagnostics.recentErrors.map((line) => `- ${line}`).join("\n")
    : "- none";
  return [
    "# Codex App Manager diagnostics",
    "",
    `Generated: ${new Date(diagnostics.generatedAtUnix * 1000).toISOString()}`,
    `Version: ${diagnostics.appVersion}`,
    `Platform: ${diagnostics.os}/${diagnostics.arch}`,
    `Update source: ${diagnostics.updateSource}`,
    `Custom source host: ${diagnostics.customSourceHost ?? "none"}`,
    `Windows install mode: ${diagnostics.windowsInstallMode ?? "n/a"}`,
    `Install status: ${diagnostics.installStatus}`,
    `Logs dir: ${diagnostics.logsDir ?? "n/a"}`,
    "",
    "## Config health",
    `Settings: ${health.settingsStatus}`,
    `Provenance: ${health.provenanceStatus}`,
    `Unknown source: ${health.unknownSource ?? "none"}`,
    `Detail: ${health.detail ?? "none"}`,
    "",
    "## Recent warnings/errors",
    recentErrors,
    "",
    "## Log tail",
    diagnostics.logTail || "(empty)",
  ].join("\n");
}

export function About({ onBack }: { onBack: () => void }) {
  const { t } = useI18n();
  const [mgrBusy, setMgrBusy] = useState(false);
  const [mgrMsg, setMgrMsg] = useState<string | null>(null);
  const [pendingUpdate, setPendingUpdate] = useState<ManagerUpdateAvailable | null>(null);

  const closeUpdateConfirm = useCallback(() => {
    if (mgrBusy) return;
    void pendingUpdate?.discard();
    setPendingUpdate(null);
  }, [mgrBusy, pendingUpdate]);

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
      setMgrMsg(errorMessage(cause));
    } finally {
      setMgrBusy(false);
    }
  }, [pendingUpdate, t]);

  const installManagerUpdate = useCallback(async () => {
    if (!pendingUpdate) return;
    setMgrBusy(true);
    setMgrMsg(t("progress.installing"));
    try {
      await pendingUpdate.installAndRelaunch();
    } catch (cause) {
      setMgrMsg(errorMessage(cause));
      setPendingUpdate(null);
    } finally {
      setMgrBusy(false);
    }
  }, [pendingUpdate, t]);

  const openLogsDir = useCallback(async () => {
    setMgrMsg(null);
    try {
      await managerApi.openLogsDir();
    } catch (cause) {
      setMgrMsg(errorMessage(cause));
    }
  }, []);

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
      <NavBar title={t("settings.more.about")} onBack={onBack} disableBack={mgrBusy} />
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
      {pendingUpdate ? (
        <div className="scrim" onClick={closeUpdateConfirm}>
          <div className="sheet" onClick={(e) => e.stopPropagation()}>
            <Ring icon="arrowUp" />
            <h3>{t("confirm.title", { version: pendingUpdate.version })}</h3>
            <p>{t("about.mgrConfirmBody")}</p>
            <div className="row2">
              <button className="btn ghost" onClick={closeUpdateConfirm} disabled={mgrBusy}>
                {t("confirm.cancel")}
              </button>
              <button className="btn primary" onClick={installManagerUpdate} disabled={mgrBusy}>
                {mgrBusy ? t("progress.installing") : t("confirm.ok")}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
