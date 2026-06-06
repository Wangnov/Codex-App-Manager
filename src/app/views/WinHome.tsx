import { useCallback, useEffect, useMemo, useState } from "react";

import { managerApi } from "../../services/managerApi";
import type {
  AppSettings,
  CapabilityCheck,
  WinAutoStageReport,
  WinInstallStatus,
  WinPerformReport,
  WinUpdateReport,
} from "../../shared/types";
import { DEFAULT_SETTINGS } from "../../shared/types";
import { Icon } from "../icons";
import { useI18n, type TKey } from "../i18n";
import { Ring, Toggle, TopBar } from "../components";

const AUTO_DOWNLOAD_KEY = "codex-manager.win.autoDownload";
const AUTO_ALLOW_METERED_KEY = "codex-manager.win.autoAllowMetered";

function mib(bytes: number): string {
  return `${(bytes / 1_048_576).toFixed(1)} MB`;
}

function readStoredBool(key: string, fallback: boolean): boolean {
  try {
    const value = localStorage.getItem(key);
    return value === null ? fallback : value === "true";
  } catch {
    return fallback;
  }
}

function writeStoredBool(key: string, value: boolean): void {
  try {
    localStorage.setItem(key, String(value));
  } catch {
    // ignore storage failures; the session state still updates
  }
}

function capabilityLabel(check: CapabilityCheck, t: (key: TKey) => string): string {
  const state =
    check.state === "available"
      ? t("win.capability.available")
      : check.state === "unavailable"
        ? t("win.capability.blocked")
        : t("win.capability.unknown");
  return `${state} · ${check.detail}`;
}

type Kind = "loading" | "error" | "none" | "idle" | "update" | "external" | "uptodate";

// Windows counterpart of MacHome — same design system + state machine, driven by
// the win_* backend (codex-win-engine): MSIX sideload or portable fallback.
export function WinHome({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { t } = useI18n();
  const [report, setReport] = useState<WinUpdateReport | null>(null);
  const [autoStage, setAutoStage] = useState<WinAutoStageReport | null>(null);
  const [status, setStatus] = useState<WinInstallStatus | null>(null);
  const [perform, setPerform] = useState<WinPerformReport | null>(null);
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [busy, setBusy] = useState<"plan" | "perform" | "adopt" | "install" | null>(null);
  const [autoBusy, setAutoBusy] = useState(false);
  const [autoDownloadEnabled, setAutoDownloadEnabled] = useState(() =>
    readStoredBool(AUTO_DOWNLOAD_KEY, true),
  );
  const [autoAllowMetered, setAutoAllowMetered] = useState(() =>
    readStoredBool(AUTO_ALLOW_METERED_KEY, false),
  );
  const [lastAutoStageKey, setLastAutoStageKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [statusLoaded, setStatusLoaded] = useState(false);
  const [statusFailed, setStatusFailed] = useState(false);

  const check = useCallback(async () => {
    setBusy("plan");
    setError(null);
    setAutoStage(null);
    setLastAutoStageKey(null);
    try {
      setReport(await managerApi.winPlanUpdate());
    } catch (cause) {
      setReport(null);
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }, []);

  useEffect(() => {
    writeStoredBool(AUTO_DOWNLOAD_KEY, autoDownloadEnabled);
  }, [autoDownloadEnabled]);

  useEffect(() => {
    writeStoredBool(AUTO_ALLOW_METERED_KEY, autoAllowMetered);
  }, [autoAllowMetered]);

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await managerApi.winStatus());
      setStatusFailed(false);
    } catch {
      setStatusFailed(true);
    } finally {
      setStatusLoaded(true);
    }
  }, []);

  useEffect(() => {
    void (async () => {
      const s = await managerApi.getSettings().catch(() => DEFAULT_SETTINGS);
      setSettings(s);
      void refreshStatus();
      if (s.autoCheck) {
        void check();
      }
    })();
  }, [check, refreshStatus]);

  const adopt = useCallback(async () => {
    setBusy("adopt");
    setError(null);
    try {
      setStatus(await managerApi.winAdopt());
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }, []);

  // Windows install + update both go through win_perform_update (the route —
  // MSIX sideload or portable fallback — is decided by the backend plan).
  const runPerform = useCallback(
    async (mode: "perform" | "install") => {
      setBusy(mode);
      setError(null);
      try {
        const result = await managerApi.winPerformUpdate(true);
        setPerform(result);
        setConfirmOpen(false);
        await refreshStatus();
        await check();
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
        setConfirmOpen(false);
      } finally {
        setBusy(null);
      }
    },
    [refreshStatus, check],
  );

  const plan = report?.plan ?? null;
  const installed = status?.installed ?? report?.installed ?? null;
  const isManaged = status?.status === "managed";
  const updateAvailable = Boolean(plan) && !plan?.upToDate;
  const routeNote =
    plan?.route === "portable-fallback" ? t("win.route.portable") : t("win.route.msix");

  const kind: Kind = useMemo(() => {
    if (!installed) {
      if (busy === "plan" || !statusLoaded) return "loading";
      if (statusFailed || error) return "error";
      return "none";
    }
    if (!statusLoaded) return "loading";
    if (status?.status === "external") return "external";
    if (busy === "plan" && !report) return "loading";
    if (error && !report) return "error";
    if (!report) return "idle";
    if (updateAvailable) return "update";
    return "uptodate";
  }, [busy, report, error, installed, updateAvailable, status, statusLoaded, statusFailed]);

  useEffect(() => {
    if (!plan || plan.upToDate || !autoDownloadEnabled || autoBusy) {
      return;
    }
    const autoKey = `${plan.packageMoniker}:${autoDownloadEnabled}:${autoAllowMetered}`;
    if (lastAutoStageKey === autoKey) {
      return;
    }
    setLastAutoStageKey(autoKey);
    setAutoBusy(true);
    void managerApi
      .winAutoStageUpdate(autoDownloadEnabled, autoAllowMetered)
      .then(setAutoStage)
      .catch((cause) => {
        setAutoStage({
          enabled: autoDownloadEnabled,
          allowMetered: autoAllowMetered,
          attempted: true,
          skipped: false,
          reason: "error",
          stage: null,
          capabilities: report?.capabilities ?? null,
          notes: [cause instanceof Error ? cause.message : String(cause)],
        });
      })
      .finally(() => setAutoBusy(false));
  }, [
    autoAllowMetered,
    autoBusy,
    autoDownloadEnabled,
    lastAutoStageKey,
    plan,
    report?.capabilities,
  ]);

  const cancelDownload = useCallback(async () => {
    setError(null);
    try {
      const cancelled = await managerApi.winCancelDownload();
      setAutoStage((current) => ({
        enabled: autoDownloadEnabled,
        allowMetered: autoAllowMetered,
        attempted: current?.attempted ?? true,
        skipped: true,
        reason: cancelled ? "cancel-requested" : "no-active-download",
        stage: current?.stage ?? null,
        capabilities: current?.capabilities ?? report?.capabilities ?? null,
        notes: [
          cancelled
            ? "Download cancellation was requested; partial bytes remain for resume."
            : "No active Windows package download was running.",
        ],
      }));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [autoAllowMetered, autoDownloadEnabled, report?.capabilities]);

  const version = installed?.version || plan?.latestVersion || "";
  const sourceLabel = t(`source.${settings.source}` as TKey);

  if (busy === "perform" || busy === "install") {
    return (
      <div className="pop">
        <TopBar />
        <div className="scroll view">
          <div className="hero" style={{ marginTop: 24 }}>
            <Ring icon="loader" spin />
            <div className="headline">
              {busy === "install" ? t("progress.installing") : t("progress.title")}
            </div>
            <div className="sub">{t("progress.downloading")}</div>
            <div className="bar">
              <div className="bar-fill" style={{ width: "62%" }} />
            </div>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="pop">
      <TopBar>
        <button className="iconbtn" title={t("home.recheck")} onClick={check} disabled={busy !== null}>
          <Icon name="refresh" />
        </button>
        <button className="iconbtn" title={t("nav.settings")} onClick={onOpenSettings}>
          <Icon name="gear" />
        </button>
      </TopBar>

      <div className="scroll view">
        {perform ? (
          <div className={`banner ${perform.success ? "ok" : "err"}`}>
            <Icon name={perform.success ? "check" : "alert"} />
            <span>{perform.message}</span>
          </div>
        ) : null}
        {error ? (
          <div className="banner err">
            <Icon name="alert" />
            <span>{error}</span>
          </div>
        ) : null}

        <section className="hero">
          {kind === "loading" ? (
            <>
              <Ring icon="loader" spin />
              <div className="headline">{t("home.checking")}</div>
            </>
          ) : kind === "error" ? (
            <>
              <Ring icon="alert" variant="danger" />
              <div className="headline">{t("home.error.title")}</div>
              <div className="desc">{t("home.error.sub")}</div>
            </>
          ) : kind === "none" ? (
            <>
              <Ring icon="download" variant="muted" />
              <div className="headline">{t("home.none.title")}</div>
              <div className="desc">{t("home.none.sub")}</div>
            </>
          ) : kind === "idle" ? (
            <>
              <Ring icon="shield" variant="muted" />
              <div className="headline">{t("home.idle.title")}</div>
              <div className="sub">{t("win.installSub", { version })}</div>
              <div className="prov">
                <span className={`dot ${isManaged ? "managed" : "external"}`} />
                {isManaged ? t("prov.managed") : t("prov.external")}
              </div>
            </>
          ) : kind === "update" ? (
            <>
              <Ring icon="arrowUp" />
              <div className="headline">{t("home.update.title")}</div>
              <div className="sub">
                <span className="ver">{plan?.latestVersion}</span>
              </div>
              <div className="flow">
                {t("home.update.flow", {
                  from: plan?.currentVersion ?? version,
                  to: plan?.latestVersion ?? "",
                })}
                {plan?.downloadSize
                  ? ` · ${t("home.update.size", { size: mib(plan.downloadSize) })}`
                  : ""}
              </div>
              <div className="microcue">
                <Icon name="shield" />
                {routeNote}
              </div>
            </>
          ) : kind === "external" ? (
            <>
              <Ring icon="shield" variant="amber" />
              <div className="headline">{t("home.external.title")}</div>
              <div className="sub">{t("win.installSub", { version })}</div>
              <div className="prov">
                <span className="dot external" />
                {t("prov.external")}
              </div>
              <div className="desc">{t("home.external.desc")}</div>
            </>
          ) : (
            <>
              <Ring icon="check" />
              <div className="headline">{t("home.uptodate.title")}</div>
              <div className="sub">{t("home.uptodate.sub", { version })}</div>
              <div className="microcue">
                <Icon name="shield" />
                {t("home.official")} · {t("home.checkedJustNow")}
              </div>
            </>
          )}
        </section>

        <div className="actions">
          {kind === "update" ? (
            <button
              className="btn primary big"
              onClick={() => (settings.askBefore ? setConfirmOpen(true) : void runPerform("perform"))}
              disabled={busy !== null}
            >
              <Icon name="download" />
              {t("home.update.cta")}
            </button>
          ) : null}
          {kind === "idle" ? (
            <button className="btn primary big" onClick={check} disabled={busy !== null}>
              <Icon name="refresh" />
              {t("home.recheck")}
            </button>
          ) : null}
          {kind === "external" ? (
            <button className="btn primary big" onClick={adopt} disabled={busy !== null}>
              <Icon name="shield" />
              {t("home.external.cta")}
            </button>
          ) : null}
          {kind === "none" ? (
            <button
              className="btn primary big"
              onClick={() => runPerform("install")}
              disabled={busy !== null}
            >
              <Icon name="download" />
              {t("home.none.cta")}
            </button>
          ) : null}
          {kind === "uptodate" ? (
            <button className="btn ghost big" onClick={check} disabled={busy !== null}>
              <Icon name="refresh" />
              {t("home.recheck")}
            </button>
          ) : null}
        </div>

        <div className="group">
          <div className="group-h">{t("win.advanced.header")}</div>
          <div className="list">
            <div className="row">
              <Icon name="download" className="ricon" />
              <span className="rtext">
                <span className="rtitle">{t("win.autoStage.title")}</span>
                <span className="rsub">{t("win.autoStage.sub")}</span>
              </span>
              <Toggle
                checked={autoDownloadEnabled}
                onChange={(v) => {
                  setAutoDownloadEnabled(v);
                  setLastAutoStageKey(null);
                  if (!v) void managerApi.winCancelDownload();
                }}
              />
            </div>
            <div className="row">
              <Icon name="info" className="ricon" />
              <span className="rtext">
                <span className="rtitle">{t("win.metered.title")}</span>
                <span className="rsub">{t("win.metered.sub")}</span>
              </span>
              <Toggle
                checked={autoAllowMetered}
                disabled={!autoDownloadEnabled}
                onChange={(v) => {
                  setAutoAllowMetered(v);
                  setLastAutoStageKey(null);
                }}
              />
            </div>
            <div className="row">
              <Icon name={autoBusy ? "loader" : "shield"} className="ricon" />
              <span className="rtext">
                <span className="rtitle">
                  {autoBusy
                    ? t("win.autoStage.running")
                    : autoStage?.stage?.installReady
                      ? t("win.autoStage.ready")
                      : autoStage?.skipped
                        ? t("win.autoStage.skipped")
                        : t("win.autoStage.idle")}
                </span>
                <span className="rsub">
                  {autoStage
                    ? `${autoStage.reason}${autoStage.notes.length ? ` · ${autoStage.notes.join(" ")}` : ""}`
                    : t("win.autoStage.waiting")}
                </span>
              </span>
              <button className="btn ghost" onClick={cancelDownload} disabled={!autoBusy}>
                {t("win.autoStage.cancel")}
              </button>
            </div>
          </div>
        </div>

        {report ? (
          <div className="group">
            <div className="group-h">{t("win.capabilities.header")}</div>
            <div className="list">
              <div className="row">
                <span className="rtext">
                  <span className="rtitle">Add-AppxPackage</span>
                  <span className="rsub">
                    {capabilityLabel(report.capabilities.addAppxPackage, t)}
                  </span>
                </span>
              </div>
              <div className="row">
                <span className="rtext">
                  <span className="rtitle">AppXSvc</span>
                  <span className="rsub">
                    {capabilityLabel(report.capabilities.appxService, t)}
                  </span>
                </span>
              </div>
              <div className="row">
                <span className="rtext">
                  <span className="rtitle">{t("win.capabilities.sideload")}</span>
                  <span className="rsub">
                    {capabilityLabel(report.capabilities.sideloadPolicy, t)}
                  </span>
                </span>
              </div>
              <div className="row">
                <span className="rtext">
                  <span className="rtitle">App Installer</span>
                  <span className="rsub">
                    {capabilityLabel(report.capabilities.appInstaller, t)}
                  </span>
                </span>
              </div>
              <div className="row">
                <span className="rtext">
                  <span className="rtitle">{t("win.capabilities.metered")}</span>
                  <span className="rsub">
                    {capabilityLabel(report.capabilities.meteredNetwork, t)}
                  </span>
                </span>
              </div>
              <div className="row">
                <Icon name="shield" className="ricon" />
                <span className="rtext">
                  <span className="rtitle">{t("win.capabilities.recommendation")}</span>
                  <span className="rsub">{routeNote}</span>
                </span>
              </div>
            </div>
          </div>
        ) : null}

        {installed ? (
          <div className="foot">
            {t("home.source", { source: sourceLabel })}
            <span>·</span>
            <button className="gobtn" onClick={onOpenSettings}>
              {t("nav.settings")}
            </button>
          </div>
        ) : null}
      </div>

      {confirmOpen && plan ? (
        <div className="scrim" onClick={() => setConfirmOpen(false)}>
          <div className="sheet" onClick={(e) => e.stopPropagation()}>
            <Ring icon="arrowUp" />
            <h3>{t("confirm.title", { version: plan.latestVersion })}</h3>
            <p>
              {t("win.confirm.body")}
              <br />
              {routeNote}
            </p>
            <div className="row2">
              <button className="btn ghost" onClick={() => setConfirmOpen(false)}>
                {t("confirm.cancel")}
              </button>
              <button className="btn primary" onClick={() => runPerform("perform")}>
                {t("confirm.ok")}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
