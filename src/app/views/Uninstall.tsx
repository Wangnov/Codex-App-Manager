import { useEffect, useId, useState } from "react";

import { managerApi } from "../../services/managerApi";
import { userErrorMessage } from "../errorCopy";
import { Icon } from "../icons";
import { useI18n } from "../i18n";
import { NavBar, Ring, Toggle, StatusBanner } from "../components";
import { codexHomeDisplay } from "../paths";
import { currentPlatform } from "../platform";
import { Sheet } from "../Sheet";

function hasTauriRuntime(): boolean {
  return (
    typeof window !== "undefined" &&
    Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
  );
}

export function Uninstall({ onBack }: { onBack: () => void }) {
  const { t } = useI18n();
  const platform = currentPlatform();
  const win = platform === "windows";
  const codexHome = codexHomeDisplay(platform);
  // Default to keeping the user's data (~/.codex on mac, %USERPROFILE%\.codex on
  // Windows). Opting out is deliberate.
  const [keepData, setKeepData] = useState(true);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pathCopied, setPathCopied] = useState(false);
  // Only a managed install may be uninstalled — mirror the backend boundary.
  const [managed, setManaged] = useState<boolean | null>(null);
  // Confirmation gate: 0 = none, 1 = first confirm, 2 = data-purge confirm
  // (only reached when the user opted out of keeping data — a 3rd tap total).
  const [confirmStep, setConfirmStep] = useState<0 | 1 | 2>(0);
  const confirm1TitleId = useId();
  const confirm1BodyId = useId();
  const confirm2TitleId = useId();
  const confirm2BodyId = useId();
  const keepDataTitleId = useId();

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
      setError(userErrorMessage(cause, t));
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

  const copyPath = async () => {
    try {
      await navigator.clipboard.writeText(codexHome);
      setPathCopied(true);
    } catch {
      // Best-effort affordance; the destructive flow itself is unaffected.
    }
  };

  return (
    <div className="pop">
      <NavBar title={t("uninstall.heading")} onBack={onBack} />
      <div className="scroll view" inert={confirmStep > 0 ? true : undefined}>
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
                  <span className="rtitle" id={keepDataTitleId}>{t("uninstall.keepData")}</span>
                  <span className="rsub">{t("uninstall.keepDataNote", { path: codexHome })}</span>
                </span>
                <Toggle
                  ariaLabelledBy={keepDataTitleId}
                  checked={keepData}
                  onChange={setKeepData}
                  disabled={busy}
                />
              </div>
            </div>

            {managed === true ? (
              <div className="list">
                <div className={`row data-path-row${keepData ? "" : " danger"}`}>
                  <span className="rtext">
                    <span className="rtitle">{t("uninstall.dataPath")}</span>
                    <span className="rsub mono path">{codexHome}</span>
                    <span className="rsub">{t("uninstall.dataAffects")}</span>
                  </span>
                  <div className="install-root-actions">
                    <button className="mini-action" onClick={copyPath}>
                      <Icon name="copy" />
                      {pathCopied ? t("uninstall.pathCopied") : t("uninstall.copyPath")}
                    </button>
                    {hasTauriRuntime() ? (
                      <button
                        className="mini-action"
                        onClick={() => void managerApi.openCodexHome().catch(() => undefined)}
                      >
                        <Icon name="folder" />
                        {t("uninstall.openDir")}
                      </button>
                    ) : null}
                  </div>
                </div>
              </div>
            ) : null}

            {managed === false ? (
              <StatusBanner tone="info">{t("uninstall.needAdopt")}</StatusBanner>
            ) : null}

            {error ? <StatusBanner tone="err">{error}</StatusBanner> : null}

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

      <Sheet
        open={confirmStep === 1}
        onDismiss={() => setConfirmStep(0)}
        labelledBy={confirm1TitleId}
        describedBy={confirm1BodyId}
        initialFocus="dismiss"
      >
        <Ring icon="trash" variant="danger" />
        <h3 id={confirm1TitleId}>{t("uninstall.confirm1.title")}</h3>
        <p id={confirm1BodyId}>
          {keepData
            ? t("uninstall.confirm1.bodyKeep", { path: codexHome })
            : t("uninstall.confirm1.bodyPurge")}
        </p>
        <div className="row2 sheet-actions">
          <button className="btn ghost" onClick={() => setConfirmStep(0)}>
            {t("uninstall.cancel")}
          </button>
          <button className="btn danger" onClick={onFirstConfirm}>
            {t("uninstall.continue")}
          </button>
        </div>
      </Sheet>

      <Sheet
        open={confirmStep === 2}
        onDismiss={() => setConfirmStep(0)}
        labelledBy={confirm2TitleId}
        describedBy={confirm2BodyId}
        initialFocus="dismiss"
      >
        <Ring icon="alert" variant="danger" />
        <h3 id={confirm2TitleId}>{t("uninstall.confirm2.title")}</h3>
        <p id={confirm2BodyId}>{t("uninstall.confirm2.body", { path: codexHome })}</p>
        <div className="row2 sheet-actions">
          <button className="btn ghost" onClick={() => setConfirmStep(0)}>
            {t("uninstall.cancel")}
          </button>
          <button className="btn danger" onClick={() => void run()}>
            {t("uninstall.purgeConfirm")}
          </button>
        </div>
      </Sheet>
    </div>
  );
}
