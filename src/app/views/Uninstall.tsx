import { useEffect, useState } from "react";

import { errorMessage, managerApi } from "../../services/managerApi";
import { Icon } from "../icons";
import { useI18n } from "../i18n";
import { NavBar, Ring, Toggle } from "../components";
import { isWindows } from "../platform";

export function Uninstall({ onBack }: { onBack: () => void }) {
  const { t } = useI18n();
  const win = isWindows();
  // Default to keeping the user's data (~/.codex on mac, %USERPROFILE%\.codex on
  // Windows). Opting out is deliberate.
  const [keepData, setKeepData] = useState(true);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Only a managed install may be uninstalled — mirror the backend boundary.
  const [managed, setManaged] = useState<boolean | null>(null);
  // Confirmation gate: 0 = none, 1 = first confirm, 2 = data-purge confirm
  // (only reached when the user opted out of keeping data — a 3rd tap total).
  const [confirmStep, setConfirmStep] = useState<0 | 1 | 2>(0);

  useEffect(() => {
    const load = win ? managerApi.winStatus() : managerApi.macStatus();
    void load.then((s) => setManaged(s.status === "managed")).catch(() => setManaged(false));
  }, [win]);

  const run = async () => {
    setConfirmStep(0);
    setBusy(true);
    setError(null);
    try {
      // mac keeps ~/.codex via keepCodexHome; win purges via purgeUserData (the
      // inverse) — both surface the backend message as the source of truth.
      if (win) {
        const r = await managerApi.winUninstall(true, !keepData);
        if (!r.success) {
          setError(r.message);
          return;
        }
        setDone(r.message);
      } else {
        const r = await managerApi.macUninstall(keepData);
        if (!r.removed) {
          setError(r.message);
          return;
        }
        setDone(r.message);
      }
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  // First confirm → run when keeping data; otherwise escalate to a second,
  // explicit data-erasure confirm before doing anything destructive.
  const onFirstConfirm = () => {
    if (keepData) {
      void run();
    } else {
      setConfirmStep(2);
    }
  };

  return (
    <div className="pop">
      <NavBar title={t("uninstall.heading")} onBack={onBack} />
      <div className="scroll view">
        {done ? (
          <>
            <section className="hero" style={{ marginTop: 16 }}>
              <Ring icon="check" variant="success" className="pop" />
              <div className="headline">{t("uninstall.heading")}</div>
              <div className="desc">{done}</div>
            </section>
            <button className="btn ghost big" onClick={onBack}>
              {t("nav.back")}
            </button>
          </>
        ) : (
          <>
            <section className="hero" style={{ marginTop: 8 }}>
              <Ring icon="trash" variant="danger" />
              <div className="headline" style={{ fontSize: 18 }}>
                {t("uninstall.heading")}
              </div>
              <div className="desc">{t("uninstall.warn")}</div>
            </section>

            <div className="list">
              <div className="row">
                <span className="rtext">
                  <span className="rtitle">{t("uninstall.keepData")}</span>
                  <span className="rsub">{t("uninstall.keepDataNote")}</span>
                </span>
                <Toggle checked={keepData} onChange={setKeepData} disabled={busy} />
              </div>
            </div>

            {managed === false ? (
              <div className="banner info">
                <Icon name="info" />
                <span>{t("uninstall.needAdopt")}</span>
              </div>
            ) : null}

            {error ? (
              <div className="banner err">
                <Icon name="alert" />
                <span>{error}</span>
              </div>
            ) : null}

            <div className="actions">
              <button
                className="btn danger big"
                onClick={() => setConfirmStep(1)}
                disabled={busy || managed !== true}
              >
                <Icon name="trash" />
                {busy ? t("uninstall.working") : t("uninstall.confirm")}
              </button>
              <button className="btn ghost" onClick={onBack} disabled={busy}>
                {t("uninstall.cancel")}
              </button>
            </div>
          </>
        )}
      </div>

      {confirmStep === 1 ? (
        <div className="scrim" onClick={() => setConfirmStep(0)}>
          <div className="sheet" onClick={(e) => e.stopPropagation()}>
            <Ring icon="trash" variant="danger" />
            <h3>{t("uninstall.confirm1.title")}</h3>
            <p>{keepData ? t("uninstall.confirm1.bodyKeep") : t("uninstall.confirm1.bodyPurge")}</p>
            <div className="row2">
              <button className="btn ghost" onClick={() => setConfirmStep(0)}>
                {t("uninstall.cancel")}
              </button>
              <button className="btn danger" onClick={onFirstConfirm}>
                {t("uninstall.continue")}
              </button>
            </div>
          </div>
        </div>
      ) : null}

      {confirmStep === 2 ? (
        <div className="scrim" onClick={() => setConfirmStep(0)}>
          <div className="sheet" onClick={(e) => e.stopPropagation()}>
            <Ring icon="alert" variant="danger" />
            <h3>{t("uninstall.confirm2.title")}</h3>
            <p>{t("uninstall.confirm2.body")}</p>
            <div className="row2">
              <button className="btn ghost" onClick={() => setConfirmStep(0)}>
                {t("uninstall.cancel")}
              </button>
              <button className="btn danger" onClick={() => void run()}>
                {t("uninstall.purgeConfirm")}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
