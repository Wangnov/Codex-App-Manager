import { useCallback, useEffect, useId, useState } from "react";

import { errorMessage, managerApi } from "../../services/managerApi";
import type { InstallProbeState, OperationOutcome } from "../../shared/types";
import { outcomeIsPartial } from "../../shared/types";
import { Icon } from "../icons";
import { useI18n } from "../i18n";
import { NavBar, Ring, Toggle } from "../components";
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
  // Install probe: Loading / Managed / External / None / Error — never treat a
  // status-query failure as "external".
  const [probe, setProbe] = useState<InstallProbeState>("loading");
  const [partialOutcome, setPartialOutcome] = useState<OperationOutcome | null>(null);
  const [retryBusy, setRetryBusy] = useState(false);
  // Confirmation gate: 0 = none, 1 = first confirm, 2 = data-purge confirm
  // (only reached when the user opted out of keeping data — a 3rd tap total).
  // 3 = post-uninstall purge_user_data ancillary retry confirm (destructive).
  const [confirmStep, setConfirmStep] = useState<0 | 1 | 2 | 3>(0);
  const confirm1TitleId = useId();
  const confirm1BodyId = useId();
  const confirm2TitleId = useId();
  const confirm2BodyId = useId();
  const purgeRetryTitleId = useId();
  const purgeRetryBodyId = useId();
  const keepDataTitleId = useId();

  const refreshProbe = useCallback(async () => {
    setProbe("loading");
    try {
      const s = win ? await managerApi.winStatus() : await managerApi.macStatus();
      if (s.status === "managed") setProbe("managed");
      else if (s.status === "external") setProbe("external");
      else setProbe("none");
    } catch {
      setProbe("error");
    }
  }, [win]);

  useEffect(() => {
    void refreshProbe();
  }, [refreshProbe]);

  const run = async () => {
    setConfirmStep(0);
    setBusy(true);
    setError(null);
    setPartialOutcome(null);
    try {
      // mac keeps ~/.codex via keepCodexHome; win purges via purgeUserData (the
      // inverse) — both surface the backend message as the source of truth.
      if (win) {
        const r = await managerApi.winUninstall(true, !keepData);
        if (r.success && outcomeIsPartial(r.outcome)) {
          setPartialOutcome(r.outcome);
          setDone(r.message);
          return;
        }
        if (!r.success) {
          setError(r.message);
          return;
        }
        setDone(r.message);
      } else {
        const r = await managerApi.macUninstall(keepData);
        if (r.removed && outcomeIsPartial(r.outcome)) {
          setPartialOutcome(r.outcome);
          setDone(r.message);
          return;
        }
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

  const retryActions = async (actions: string[]) => {
    setConfirmStep(0);
    setRetryBusy(true);
    setError(null);
    try {
      const report = await managerApi.retryAncillary({
        actions,
        path: partialOutcome?.path ?? null,
        purgeUserData: actions.includes("purge_user_data"),
      });
      setDone(report.message);
      if (outcomeIsPartial(report.outcome)) {
        setPartialOutcome(report.outcome);
      } else {
        setPartialOutcome(null);
      }
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setRetryBusy(false);
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

  const canUninstall = probe === "managed";

  return (
    <div className="pop">
      <NavBar title={t("uninstall.heading")} onBack={onBack} />
      <div className="scroll view" inert={confirmStep > 0 ? true : undefined}>
        {done ? (
          <>
            <section className="hero" style={{ marginTop: 16 }}>
              <Ring
                icon={partialOutcome ? "alert" : "check"}
                variant={partialOutcome ? "amber" : "success"}
                className="pop"
              />
              <div className="headline">{t("uninstall.heading")}</div>
              <div className="desc">{done}</div>
            </section>
            {partialOutcome ? (
              <>
                <div className="banner warn">
                  <Icon name="alert" />
                  <span>{t("uninstall.partial.title")}</span>
                </div>
                <div className="actions">
                  {partialOutcome.recoveryActions.includes("cleanup_metadata") ? (
                    <button
                      className="btn big"
                      disabled={retryBusy}
                      onClick={() => void retryActions(["cleanup_metadata"])}
                    >
                      {t("uninstall.partial.retryCleanup")}
                    </button>
                  ) : null}
                  {partialOutcome.recoveryActions.includes("clear_provenance") ? (
                    <button
                      className="btn big"
                      disabled={retryBusy}
                      onClick={() => void retryActions(["clear_provenance"])}
                    >
                      {t("uninstall.partial.retryProvenance")}
                    </button>
                  ) : null}
                  {partialOutcome.recoveryActions.includes("purge_user_data") ? (
                    <button
                      className="btn big"
                      disabled={retryBusy}
                      onClick={() => setConfirmStep(3)}
                    >
                      {t("uninstall.partial.retryPurge")}
                    </button>
                  ) : null}
                  {partialOutcome.recoveryActions.includes("record_provenance") ? (
                    <button
                      className="btn big"
                      disabled={retryBusy}
                      onClick={() => void retryActions(["record_provenance"])}
                    >
                      {t("uninstall.partial.retryRecord")}
                    </button>
                  ) : null}
                </div>
              </>
            ) : null}
            {error ? (
              <div className="banner err">
                <Icon name="alert" />
                <span>{error}</span>
              </div>
            ) : null}
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

            {probe === "loading" ? (
              <div className="banner info" role="status">
                <Icon name="loader" />
                <span>{t("uninstall.status.loading")}</span>
              </div>
            ) : null}

            {probe === "error" ? (
              <div className="banner err">
                <Icon name="alert" />
                <span>{t("uninstall.status.error")}</span>
                <button className="linkbtn" onClick={() => void refreshProbe()}>
                  {t("settings.retry")}
                </button>
              </div>
            ) : null}

            {probe === "none" ? (
              <div className="banner info">
                <Icon name="info" />
                <span>{t("uninstall.status.none")}</span>
              </div>
            ) : null}

            {probe === "external" ? (
              <div className="banner info">
                <Icon name="info" />
                <span>{t("uninstall.needAdopt")}</span>
              </div>
            ) : null}

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
                  disabled={busy || !canUninstall}
                />
              </div>
            </div>

            {probe === "managed" ? (
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
                disabled={busy || !canUninstall}
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
        <div className="row2">
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
        <Ring icon="trash" variant="danger" />
        <h3 id={confirm2TitleId}>{t("uninstall.confirm2.title")}</h3>
        <p id={confirm2BodyId}>{t("uninstall.confirm2.body", { path: codexHome })}</p>
        <div className="row2">
          <button className="btn ghost" onClick={() => setConfirmStep(0)}>
            {t("uninstall.cancel")}
          </button>
          <button className="btn danger" onClick={() => void run()}>
            {t("uninstall.purgeConfirm")}
          </button>
        </div>
      </Sheet>

      {/* Destructive: retry purge_user_data only — same consequence copy as full purge. */}
      <Sheet
        open={confirmStep === 3}
        onDismiss={() => setConfirmStep(0)}
        labelledBy={purgeRetryTitleId}
        describedBy={purgeRetryBodyId}
        initialFocus="dismiss"
      >
        <Ring icon="trash" variant="danger" />
        <h3 id={purgeRetryTitleId}>{t("uninstall.confirm2.title")}</h3>
        <p id={purgeRetryBodyId}>{t("uninstall.confirm2.body", { path: codexHome })}</p>
        <div className="row2">
          <button className="btn ghost" onClick={() => setConfirmStep(0)}>
            {t("uninstall.cancel")}
          </button>
          <button
            className="btn danger"
            onClick={() => void retryActions(["purge_user_data"])}
          >
            {t("uninstall.partial.retryPurge")}
          </button>
        </div>
      </Sheet>
    </div>
  );
}
