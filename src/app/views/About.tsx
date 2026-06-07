import { useCallback, useState } from "react";

import { errorMessage, managerApi } from "../../services/managerApi";
import { Icon, CodexMark } from "../icons";
import { useI18n } from "../i18n";
import { NavBar } from "../components";

const APP_VERSION = "0.1.0";
const REPO_URL = "https://github.com/Wangnov/Codex-App-Manager";

export function About({ onBack }: { onBack: () => void }) {
  const { t } = useI18n();
  const [mgrBusy, setMgrBusy] = useState(false);
  const [mgrMsg, setMgrMsg] = useState<string | null>(null);

  const checkManager = useCallback(async () => {
    setMgrBusy(true);
    setMgrMsg(null);
    try {
      setMgrMsg(await managerApi.checkManagerUpdate());
    } catch (cause) {
      setMgrMsg(errorMessage(cause));
    } finally {
      setMgrBusy(false);
    }
  }, []);

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
        </div>
      </div>
    </div>
  );
}
