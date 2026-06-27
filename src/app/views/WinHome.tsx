import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Pause, Play, XCircle } from "lucide-react";

import {
  errorCode,
  errorMessage,
  isDownloadCancelled,
  managerApi,
  SETTINGS_CHANGED_EVENT,
} from "../../services/managerApi";
import type {
  AppSettings,
  DownloadProgress,
  WinInstallStatus,
  WinPerformReport,
  WinUpdateReport,
} from "../../shared/types";
import { DEFAULT_SETTINGS } from "../../shared/types";
import { userErrorMessage } from "../errorCopy";
import { Icon, CodexGlyph } from "../icons";
import { useI18n, dirOf, type TKey } from "../i18n";
import { Ring, TopBar, ResultBanner, ErrorHero } from "../components";
import { useCountUp } from "../useCountUp";
import { mib, fmtDateTime } from "../format";
import { useHomeMotion } from "../motion";
import { Sheet } from "../Sheet";
import { skippedUpdateMatches, winSkippedUpdateCandidate } from "../skippedUpdate";
import {
  ManualExistingInstallSheet,
  type ManualExistingCandidate,
} from "./ManualExistingInstall";

function samePath(a: string, b: string): boolean {
  const norm = (value: string) =>
    value.trim().replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();
  return norm(a) === norm(b);
}

type Kind = "loading" | "error" | "none" | "idle" | "update" | "external" | "uptodate";
type DownloadStopIntent = "pause" | "cancel";

// Windows counterpart of MacHome — same design system + state machine, driven by
// the win_* backend (codex-win-engine): MSIX sideload or portable fallback.
export function WinHome({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { t, lang } = useI18n();
  const [report, setReport] = useState<WinUpdateReport | null>(null);
  const [status, setStatus] = useState<WinInstallStatus | null>(null);
  const [perform, setPerform] = useState<WinPerformReport | null>(null);
  // Version pair captured at update time (fresh installs have no "from"), so the
  // outcome strip can read "X → Y" like the mac side.
  const [updatedVer, setUpdatedVer] = useState<{ from: string; to: string } | null>(null);
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [defaultInstallRoot, setDefaultInstallRoot] = useState(DEFAULT_SETTINGS.installRoot);
  const [busy, setBusy] = useState<"plan" | "perform" | "adopt" | "install" | null>(null);
  const [checkError, setCheckError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [installDirOpen, setInstallDirOpen] = useState(false);
  const [installDirBusy, setInstallDirBusy] = useState(false);
  const [manualExistingOpen, setManualExistingOpen] = useState(false);
  const [manualExistingCandidate, setManualExistingCandidate] =
    useState<ManualExistingCandidate | null>(null);
  const [manualExistingBusy, setManualExistingBusy] = useState<"pick" | "adopt" | null>(null);
  const [manualExistingError, setManualExistingError] = useState<string | null>(null);
  const [statusLoaded, setStatusLoaded] = useState(false);
  const [statusFailed, setStatusFailed] = useState(false);
  const [dl, setDl] = useState<DownloadProgress | null>(null);
  const [speed, setSpeed] = useState(0);
  const dlSample = useRef<{ t: number; bytes: number } | null>(null);
  const [downloadStop, setDownloadStop] = useState<DownloadStopIntent | null>(null);
  const [downloadStopBusy, setDownloadStopBusy] = useState(false);
  const downloadStopRef = useRef<DownloadStopIntent | null>(null);
  // Latest live progress, read at pause time to snapshot the paused figures.
  const dlRef = useRef<DownloadProgress | null>(null);
  // A paused download: the progress screen stays up (not routed home) offering
  // 〔继续〕/〔取消〕. `installRoot` is preserved so a paused fresh install
  // resumes into the same chosen location.
  const [paused, setPaused] = useState<{
    kind: "perform" | "install";
    dl: DownloadProgress | null;
    installRoot?: string;
  } | null>(null);
  const scopeRef = useRef<HTMLDivElement>(null);
  const confirmTitleId = useId();
  const confirmBodyId = useId();
  const installDirTitleId = useId();
  const installDirBodyId = useId();
  const manualExistingTitleId = useId();
  const manualExistingBodyId = useId();
  // Smoothly roll the live download figures instead of snapping per event.
  const dlPctTarget = dl && dl.total > 0 ? Math.min(100, (dl.downloaded / dl.total) * 100) : 0;
  const dlPct = useCountUp(dlPctTarget);
  const dlBytes = useCountUp(dl?.downloaded ?? 0);
  const dlSpeed = useCountUp(speed);

  const onDlProgress = useCallback((event: { payload: DownloadProgress }) => {
    const p = event.payload;
    setDl(p);
    dlRef.current = p;
    const now = Date.now();
    const prev = dlSample.current;
    if (!prev) {
      dlSample.current = { t: now, bytes: p.downloaded };
    } else if (now > prev.t + 400) {
      setSpeed((p.downloaded - prev.bytes) / ((now - prev.t) / 1000));
      dlSample.current = { t: now, bytes: p.downloaded };
    }
  }, []);

  const startDlListen = useCallback(async () => {
    setDl(null);
    dlRef.current = null;
    setSpeed(0);
    dlSample.current = null;
    try {
      return await listen<DownloadProgress>("win://download-progress", onDlProgress);
    } catch {
      return () => {};
    }
  }, [onDlProgress]);

  const requestDownloadStop = useCallback(
    async (intent: DownloadStopIntent) => {
      setActionError(null);
      setDownloadStop(intent);
      setDownloadStopBusy(true);
      downloadStopRef.current = intent;
      try {
        const active =
          intent === "pause"
            ? await managerApi.winPauseDownload()
            : await managerApi.winCancelDownload();
        if (!active) {
          downloadStopRef.current = null;
          setDownloadStop(null);
          setDownloadStopBusy(false);
          setActionError(t("progress.cannotCancel"));
        }
      } catch (cause) {
        downloadStopRef.current = null;
        setDownloadStop(null);
        setDownloadStopBusy(false);
        setActionError(userErrorMessage(cause, t));
      }
    },
    [t],
  );

  const check = useCallback(async () => {
    setBusy("plan");
    setCheckError(null);
    setActionError(null);
    setNotice(null);
    try {
      setReport(await managerApi.winPlanUpdate());
      return true;
    } catch (cause) {
      setReport(null);
      setCheckError(errorMessage(cause));
      return false;
    } finally {
      setBusy(null);
    }
  }, []);

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
      void managerApi.winDefaultInstallRoot().then(setDefaultInstallRoot).catch(() => undefined);
      void refreshStatus();
      if (s.checkOnStartup) {
        void check();
      }
    })();
  }, [check, refreshStatus]);

  useEffect(() => {
    const onSettingsChanged = (event: Event) => {
      setSettings((event as CustomEvent<AppSettings>).detail);
    };
    window.addEventListener(SETTINGS_CHANGED_EVENT, onSettingsChanged);
    return () => window.removeEventListener(SETTINGS_CHANGED_EVENT, onSettingsChanged);
  }, []);

  const busyRef = useRef(busy);
  useEffect(() => {
    busyRef.current = busy;
  }, [busy]);
  const checkRef = useRef(check);
  useEffect(() => {
    checkRef.current = check;
  }, [check]);

  useEffect(() => {
    if (!settings.periodicCheck) return;
    const intervalMs = Math.max(60_000, settings.periodicCheckIntervalSeconds * 1000);
    const id = window.setInterval(() => {
      if (busyRef.current) return;
      void checkRef.current();
    }, intervalMs);
    return () => window.clearInterval(id);
  }, [settings.periodicCheck, settings.periodicCheckIntervalSeconds]);

  const adopt = useCallback(async () => {
    setBusy("adopt");
    setCheckError(null);
    setActionError(null);
    setNotice(null);
    try {
      setStatus(await managerApi.winAdopt());
    } catch (cause) {
      setActionError(userErrorMessage(cause, t));
    } finally {
      setBusy(null);
    }
  }, [t]);

  // The probe recommended MSIX, but this PC looks like it's missing the Store /
  // App Installer components — the MSIX can install yet fail to launch (the very
  // issue users hit). Let them switch to the portable build in one tap: persist
  // the preference, then re-plan so the route flips to portable and this notice
  // clears.
  const switchToPortable = useCallback(async () => {
    setActionError(null);
    try {
      const next: AppSettings = { ...settings, windowsInstallMode: "portable" };
      setSettings(await managerApi.setSettings(next));
    } catch (cause) {
      setActionError(userErrorMessage(cause, t));
      return;
    }
    await check();
  }, [settings, check, t]);

  // Windows install + update both go through win_perform_update (the route —
  // MSIX sideload or portable fallback — is decided by the backend plan).
  const runPerform = useCallback(
    async (mode: "perform" | "install", installRoot?: string) => {
      setBusy(mode);
      setActionError(null);
      setNotice(null);
      setPaused(null);
      // For an in-place update (not a fresh install) capture the human-facing
      // versions before the swap, so the outcome strip can show "X → Y".
      // Prefer the report (one atomic snapshot of installed + plan) so the
      // strip can't pair a stale installed version with a fresh plan.
      const fromVersion =
        mode === "perform" ? report?.installed?.version ?? status?.installed?.version ?? "" : "";
      const toVersion = report?.plan?.latestVersion ?? "";
      const unlisten = await startDlListen();
      try {
        const expected = report?.plan
          ? {
              currentVersion: report.plan.currentVersion,
              latestVersion: report.plan.latestVersion,
              packageMoniker: report.plan.packageMoniker,
              route: report.plan.route,
            }
          : undefined;
        const result = await managerApi.winPerformUpdate(true, expected, installRoot);
        setPerform(result);
        setUpdatedVer(
          mode === "perform" && fromVersion && toVersion
            ? { from: fromVersion, to: toVersion }
            : null,
        );
        setConfirmOpen(false);
        setInstallDirOpen(false);
        await refreshStatus();
        await check();
      } catch (cause) {
        setConfirmOpen(false);
        setInstallDirOpen(false);
        const stop = downloadStopRef.current;
        if (stop === "pause" && isDownloadCancelled(cause)) {
          // Stay on the progress screen as paused; the cached `.part` lets
          // 〔继续〕 resume from here (with the same install location).
          setPaused({ kind: mode, dl: dlRef.current, installRoot });
        } else if (stop && isDownloadCancelled(cause)) {
          setNotice(t("progress.cancelled"));
        } else if (errorCode(cause) === "stale_expectation") {
          await refreshStatus();
          if (await check()) {
            setNotice(t("home.stale.rechecked"));
          }
        } else {
          setActionError(userErrorMessage(cause, t));
        }
      } finally {
        unlisten();
        setBusy(null);
        setDl(null);
        setDownloadStop(null);
        setDownloadStopBusy(false);
        downloadStopRef.current = null;
      }
    },
    [status, report, refreshStatus, check, startDlListen, t],
  );

  // 〔继续〕from the paused state — re-run the same operation (same install
  // location). The backend finds the cached `.part` and resumes via `curl -C -`,
  // so the bar picks up where it stopped instead of at 0.
  const resumeDownload = useCallback(() => {
    const snapshot = paused;
    setPaused(null);
    if (!snapshot) return;
    void runPerform(snapshot.kind, snapshot.installRoot);
  }, [paused, runPerform]);

  // 〔取消〕from the paused state — the download already stopped, so drop the
  // cached partial and route home.
  const cancelPausedDownload = useCallback(async () => {
    setPaused(null);
    try {
      // Only claim "已取消" once the cached partial is actually gone — otherwise
      // a failed discard would leave a `.part` that the next update silently
      // resumes, contradicting the cancel.
      await managerApi.winDiscardDownload();
      setNotice(t("progress.cancelled"));
    } catch (cause) {
      setActionError(userErrorMessage(cause, t));
    }
  }, [t]);

  const freshInstallNeedsLocation = useCallback(async () => {
    if (settings.windowsInstallMode === "portable" || report?.plan?.route === "portable-fallback") {
      return true;
    }
    if (report?.plan?.route === "msix-sideload") {
      return false;
    }
    setBusy("plan");
    setCheckError(null);
    setActionError(null);
    try {
      const next = await managerApi.winPlanUpdate();
      setReport(next);
      return next.plan?.route === "portable-fallback";
    } catch (cause) {
      setCheckError(errorMessage(cause));
      return null;
    } finally {
      setBusy(null);
    }
  }, [report?.plan?.route, settings.windowsInstallMode]);

  const requestInstall = useCallback(async () => {
    const needsLocation = await freshInstallNeedsLocation();
    if (needsLocation === null) {
      return;
    }
    if (needsLocation) {
      setInstallDirOpen(true);
      return;
    }
    await runPerform("install");
  }, [freshInstallNeedsLocation, runPerform]);

  const installToCurrentRoot = useCallback(async () => {
    await runPerform("install", settings.installRoot);
  }, [runPerform, settings.installRoot]);

  const browseInstallRoot = useCallback(async () => {
    setInstallDirBusy(true);
    setActionError(null);
    try {
      const path = await managerApi.winPickInstallDir();
      if (!path) return;
      // One-shot: hand the chosen location straight to the install. The backend
      // only persists it as the new default after the install succeeds, so a
      // cancelled or failed attempt leaves the saved location untouched. Refresh
      // settings afterwards to reflect whatever was (or wasn't) persisted.
      await runPerform("install", path);
      const refreshed = await managerApi.getSettings().catch(() => null);
      if (refreshed) setSettings(refreshed);
    } catch (cause) {
      setActionError(userErrorMessage(cause, t));
      setInstallDirOpen(false);
    } finally {
      setInstallDirBusy(false);
    }
  }, [runPerform, t]);

  const plan = report?.plan ?? null;
  const installed = report ? report.installed : status?.installed ?? null;
  const statusMatchesInstalled = Boolean(
    installed &&
      status?.installed &&
      samePath(installed.path, status.installed.path) &&
      installed.version === status.installed.version,
  );
  const isManaged = statusMatchesInstalled && status?.status === "managed";
  const skippedCandidate = useMemo(() => winSkippedUpdateCandidate(plan), [plan]);
  const updateSuppressed = skippedUpdateMatches(settings.skippedCodexUpdate, skippedCandidate);
  const updateAvailable = Boolean(plan) && !plan?.upToDate && !updateSuppressed;
  const routeNote =
    plan?.route === "portable-fallback" ? t("win.route.portable") : t("win.route.msix");
  // MSIX is the planned route, yet the Desktop App Installer wasn't detected —
  // a stripped Windows where the package may install but not launch. The probe
  // only ever reports appInstaller as "available" or "unknown" (never
  // "unavailable"), so "unknown" is the not-detected signal we gate on.
  const msixRisky =
    plan?.route === "msix-sideload" &&
    report?.capabilities?.appInstaller?.state === "unknown";

  const kind: Kind = useMemo(() => {
    if (!installed) {
      if (busy === "plan" || !statusLoaded) return "loading";
      if (statusFailed || checkError) return "error";
      return "none";
    }
    if (!statusLoaded) return "loading";
    if (statusMatchesInstalled && status?.status === "external") return "external";
    if (busy === "plan" && !report) return "loading";
    if (checkError && !report) return "error";
    if (!report) return "idle";
    if (updateSuppressed) return "idle";
    if (updateAvailable) return "update";
    return "uptodate";
  }, [
    busy,
    report,
    checkError,
    installed,
    updateSuppressed,
    updateAvailable,
    status,
    statusMatchesInstalled,
    statusLoaded,
    statusFailed,
  ]);

  const version = installed?.version || plan?.latestVersion || "";
  const sourceLabel = t(`source.${settings.source}` as TKey);
  const installRootIsDefault = samePath(settings.installRoot, defaultInstallRoot);

  // A re-check (or the first auto-check) while an app is already known: the hero
  // morphs to the checking state so the status visibly reacts, then settles back.
  const rechecking = busy === "plan" && Boolean(installed);
  // Windows release time is shown only when it describes the installed/current
  // version. If the manifest omits it, skip the date row rather than showing an
  // install timestamp.
  const packageReleaseDate = fmtDateTime(report?.release.releasedAt ?? null, lang);
  const latestReleaseDate = updateAvailable ? packageReleaseDate : null;
  const releaseDate = plan?.upToDate ? packageReleaseDate : null;
  const updateSize =
    updateAvailable && plan?.downloadSize != null
      ? t("home.update.size", { size: mib(plan.downloadSize) })
      : null;

  const openManualExisting = useCallback(() => {
    setManualExistingError(null);
    setManualExistingOpen(true);
  }, []);

  const closeManualExisting = useCallback(() => {
    if (manualExistingBusy) return;
    setManualExistingOpen(false);
    setManualExistingError(null);
  }, [manualExistingBusy]);

  const pickManualExisting = useCallback(async () => {
    setManualExistingBusy("pick");
    setManualExistingError(null);
    try {
      const selected = await managerApi.winPickExistingInstall();
      if (selected) {
        setManualExistingCandidate({
          path: selected.path,
          version: selected.version,
          releaseDate: releaseDate ?? null,
        });
      }
    } catch (cause) {
      setManualExistingError(errorMessage(cause));
    } finally {
      setManualExistingBusy(null);
    }
  }, [releaseDate]);

  const adoptManualExisting = useCallback(async () => {
    if (!manualExistingCandidate) return;
    setManualExistingBusy("adopt");
    setManualExistingError(null);
    try {
      const next = await managerApi.winAdoptPath(manualExistingCandidate.path);
      setStatus(next);
      setStatusLoaded(true);
      setStatusFailed(false);
      setManualExistingOpen(false);
      setManualExistingCandidate(null);
      await check();
    } catch (cause) {
      setManualExistingError(errorMessage(cause));
    } finally {
      setManualExistingBusy(null);
    }
  }, [check, manualExistingCandidate]);

  const skipCurrentUpdate = useCallback(async () => {
    if (!skippedCandidate) return;
    setActionError(null);
    try {
      const saved = await managerApi.setSettings({
        ...settings,
        skippedCodexUpdate: { ...skippedCandidate, skippedAt: Date.now() },
      });
      setSettings(saved);
      setNotice(t("home.skip.toast", { version: skippedCandidate.version }));
    } catch (cause) {
      setActionError(userErrorMessage(cause, t));
    }
  }, [settings, skippedCandidate, t]);
  const onLaunch = () => {
    // Surface a failed open (PowerShell/AUMID or portable-exe error) via the
    // error banner like every other action, not an unhandled rejection.
    setActionError(null);
    void managerApi.winLaunch().catch((cause) => setActionError(userErrorMessage(cause, t)));
  };

  // Scene id; on change the hero remounts and GSAP replays the entrance. `lang`
  // is part of the key so a language switch re-splits the headline (otherwise
  // SplitText's aria-label keeps the old language's text for screen readers).
  const progressing = busy === "perform" || busy === "install";
  // The paused screen is calm (no shimmer): a settled "已暂停", not in-flight.
  const isShimmer = progressing || rechecking || kind === "loading";
  const scene = `${lang}/${
    paused
      ? `paused-${paused.kind}`
      : progressing
        ? `progress-${busy}`
        : `${kind}${rechecking ? "-checking" : ""}`
  }`;
  const success = !rechecking && kind === "uptodate";
  // A Windows install/update is "clean" only when it actually changed something
  // without a detour — not a stale-plan no-op (stage.upToDate) and not an
  // MSIX→portable fallback. Non-clean successes keep the backend's explanation
  // (message + notes) and stay pinned; only clean ones self-dismiss.
  const winClean =
    Boolean(perform?.success) && !perform?.stage?.upToDate && !perform?.fallbackAttempted;
  const winResultDetail =
    perform && !winClean ? perform.notes.filter(Boolean).join(" · ") || undefined : undefined;
  // Char-split only LTR scripts — splitting cursive RTL (Arabic) breaks joining.
  const splitHeadline = !isShimmer && dirOf(lang) === "ltr";
  useHomeMotion(scopeRef, scene, { splitHeadline, success });

  if (progressing || paused) {
    const installing = paused ? paused.kind === "install" : busy === "install";
    const snap = paused ? paused.dl : dl;
    const known = Boolean(snap && snap.total > 0);
    const snapPct = snap && snap.total > 0 ? Math.min(100, (snap.downloaded / snap.total) * 100) : 0;
    const pct = known ? Math.round(paused ? snapPct : dlPct) : null;
    const barPct = paused ? snapPct : dlPct;
    // Bytes are in → the uninterruptible install phase (sideload / portable
    // extract). Say so and drop the dead buttons rather than grey them silently.
    const finishing = !paused && Boolean(snap && snap.total > 0 && snap.downloaded >= snap.total);
    // Pause only mid-transfer; cancel is the "abandon" out and works through the
    // preparing phase too (a backend abort checkpoint honors it), but not once
    // the install has begun.
    const canPause =
      !paused && Boolean(dl && dl.total > 0 && dl.downloaded < dl.total) && !downloadStopBusy;
    const canCancel = !paused && !finishing && !downloadStopBusy;
    return (
      <div className="pop">
        <TopBar />
        <div className="scroll" ref={scopeRef}>
          <div className="hero" style={{ marginTop: 24 }} key={scene}>
            <Ring icon={paused ? "pause" : "loader"} spin={!paused} className="glow" />
            <div className={`headline${paused ? "" : " shimmer"}`}>
              {paused
                ? t("progress.paused.title")
                : installing
                  ? t("progress.installing")
                  : t("progress.title")}
            </div>
            {!paused ? (
              <div className="sub">
                {finishing
                  ? t("progress.finishing")
                  : snap
                    ? t("progress.downloadingFrom", { source: snap.source })
                    : t("progress.preparing")}
              </div>
            ) : null}
            {pct !== null ? (
              <div className="pctbig">
                {pct}
                <span className="pctsign">%</span>
              </div>
            ) : null}
            <div className="bar">
              <div
                className={`bar-fill${pct === null ? " indeterminate" : ""}`}
                style={pct === null ? undefined : { width: `${barPct}%` }}
              />
            </div>
            {known && snap ? (
              <div className="dlmeta">
                {mib(paused ? snap.downloaded : dlBytes)} / {mib(snap.total)}
                {!paused && dlSpeed > 0 ? ` · ${mib(dlSpeed)}/s` : ""}
              </div>
            ) : null}
            <div className="progress-actions">
              {paused ? (
                <button className="btn primary" onClick={resumeDownload}>
                  <Play />
                  {t("progress.resume")}
                </button>
              ) : (
                <button
                  className="btn ghost"
                  onClick={() => void requestDownloadStop("pause")}
                  disabled={!canPause}
                >
                  <Pause />
                  {downloadStop === "pause" ? t("progress.pausePending") : t("progress.pause")}
                </button>
              )}
              <button
                className="btn danger"
                onClick={() => {
                  if (paused) cancelPausedDownload();
                  else void requestDownloadStop("cancel");
                }}
                disabled={!paused && !canCancel}
              >
                <XCircle />
                {downloadStop === "cancel" ? t("progress.cancelPending") : t("progress.cancel")}
              </button>
            </div>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="pop">
      <TopBar>
        <button className="iconbtn" title={t("nav.settings")} onClick={onOpenSettings}>
          <Icon name="gear" />
        </button>
      </TopBar>

      <div
        className="scroll"
        ref={scopeRef}
        inert={confirmOpen || installDirOpen || manualExistingOpen ? true : undefined}
      >
        {perform ? (
          <ResultBanner
            tone={perform.success ? "ok" : "err"}
            // Clean success → the version bump / install line. Anything non-clean
            // (no-op, fallback, or failure) keeps the backend's message so the
            // reason is never lost.
            title={
              winClean
                ? updatedVer
                  ? t("success.flow", { from: updatedVer.from, to: updatedVer.to })
                  : t("install.done.title")
                : perform.message
            }
            detail={winResultDetail}
            autoDismissMs={winClean ? 6000 : undefined}
            onClose={() => {
              setPerform(null);
              setUpdatedVer(null);
            }}
          />
        ) : null}
        {notice ? (
          <div className="banner info">
            <Icon name="info" />
            <span>{notice}</span>
          </div>
        ) : null}
        {actionError ? (
          <div className="banner err">
            <Icon name="alert" />
            <span>{actionError}</span>
          </div>
        ) : null}

        <section className="hero" key={scene}>
          {rechecking ? (
            // Mirror the settled hero's line count (ring + headline + status
            // line) so nothing below shifts while the check runs.
            <>
              <Ring icon="loader" spin className="glow" />
              <div className="headline shimmer">{t("home.checking")}</div>
              <div className="microcue" style={{ visibility: "hidden" }} aria-hidden="true">
                <Icon name="shield" />
                {t("home.official")}
              </div>
            </>
          ) : kind === "loading" ? (
            <>
              <Ring icon="loader" spin className="glow" />
              <div className="headline shimmer">{t("home.checking")}</div>
            </>
          ) : kind === "error" ? (
            <ErrorHero message={checkError} />
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
              <div className="prov">
                <span className={`dot ${isManaged ? "managed" : "external"}`} />
                {isManaged ? t("prov.managed") : t("prov.external")}
              </div>
            </>
          ) : kind === "update" ? (
            <>
              <Ring icon="arrowUp" className="glow" />
              <div className="headline">{t("home.update.title")}</div>
            </>
          ) : kind === "external" ? (
            <>
              <Ring icon="shield" variant="amber" />
              <div className="headline">{t("home.external.title")}</div>
              <div className="prov">
                <span className="dot external" />
                {t("prov.external")}
              </div>
              <div className="desc">{t("home.external.desc")}</div>
            </>
          ) : (
            <>
              <Ring icon="check" variant="success" />
              <div className="headline">{t("home.uptodate.title")}</div>
              <div className="microcue">
                <Icon name="shield" />
                {t("home.official")} · {t("home.checkedJustNow")}
              </div>
            </>
          )}
        </section>

        {/* Installed-version details — the version/date/path share one hierarchy. */}
        {installed && (rechecking || kind !== "loading") ? (
          <div className="list meta">
            {updateAvailable && plan?.latestVersion ? (
              <div className="row update-meta">
                <span className="rtext">
                  <span className="rtitle">{t("home.update.title")}</span>
                </span>
                <span className="rval version latest">{plan.latestVersion}</span>
              </div>
            ) : null}
            {updateAvailable && latestReleaseDate ? (
              <div className="row update-meta">
                <span className="rtext">
                  <span className="rtitle">{t("home.releaseDate")}</span>
                </span>
                <span className="rval latest">{latestReleaseDate}</span>
              </div>
            ) : null}
            {updateAvailable && updateSize ? (
              <div className="row update-meta">
                <span className="rtext">
                  <span className="rtitle">{t("home.updateSize")}</span>
                </span>
                <span className="rval latest">{updateSize}</span>
              </div>
            ) : null}
            {version ? (
              <div className="row">
                <span className="rtext">
                  <span className="rtitle">{t("home.currentVersion")}</span>
                </span>
                <span className="rval version">{version}</span>
              </div>
            ) : null}
            {releaseDate ? (
              <div className="row">
                <span className="rtext">
                  <span className="rtitle">{t("home.releaseDate")}</span>
                </span>
                <span className="rval">{releaseDate}</span>
              </div>
            ) : null}
            <div className="row">
              <span className="rtext">
                <span className="rtitle">{t("home.installLocation")}</span>
              </span>
              <span className="rval path" title={installed.path}>
                {installed.path}
              </span>
            </div>
          </div>
        ) : null}

        {!rechecking && msixRisky && (kind === "none" || kind === "update") ? (
          <div className="banner warn">
            <Icon name="alert" />
            <span>{t("win.msixRisk.body")}</span>
            <button className="linkbtn" onClick={switchToPortable} disabled={busy !== null}>
              {t("win.msixRisk.switch")}
            </button>
          </div>
        ) : null}

        <div className={`actions${!rechecking && kind === "update" ? " update-actions" : ""}`}>
          {/* While a check runs we keep a STABLE pair of buttons so nothing
              reflows under the hero. */}
          {rechecking ? (
            <>
              <button className="btn primary big" onClick={onLaunch} disabled>
                <CodexGlyph />
                {t("home.launch")}
              </button>
              <button className="btn ghost" disabled>
                <Icon name="loader" className="spinicon" />
                {t("home.checking")}
              </button>
            </>
          ) : null}
          {!rechecking && kind === "update" ? (
            <>
              <button className="btn ghost big" onClick={check} disabled={busy !== null}>
                <Icon name="refresh" />
                {t("home.recheck")}
              </button>
              <button
                className="btn primary big"
                onClick={() => (settings.askBefore ? setConfirmOpen(true) : void runPerform("perform"))}
                disabled={busy !== null}
              >
                <Icon name="download" />
                {t("home.update.cta")}
              </button>
            </>
          ) : null}
          {!rechecking && kind === "idle" ? (
            <>
              <button className="btn primary big" onClick={onLaunch} disabled={busy !== null}>
                <CodexGlyph />
                {t("home.launch")}
              </button>
              <button className="btn ghost" onClick={check} disabled={busy !== null}>
                <Icon name="refresh" />
                {t("home.recheck")}
              </button>
            </>
          ) : null}
          {!rechecking && kind === "external" ? (
            <>
              <button className="btn primary big" onClick={adopt} disabled={busy !== null}>
                <Icon name="shield" />
                {t("home.external.cta")}
              </button>
              <button className="btn ghost" onClick={onLaunch} disabled={busy !== null}>
                <CodexGlyph />
                {t("home.launch")}
              </button>
            </>
          ) : null}
          {!rechecking && kind === "none" ? (
            <button className="btn primary big" onClick={requestInstall} disabled={busy !== null}>
              <Icon name="download" />
              {t("home.none.cta")}
            </button>
          ) : null}
          {!rechecking && kind === "uptodate" ? (
            <>
              <button className="btn primary big" onClick={onLaunch} disabled={busy !== null}>
                <CodexGlyph />
                {t("home.launch")}
              </button>
              <button className="btn ghost" onClick={check} disabled={busy !== null}>
                <Icon name="refresh" />
                {t("home.recheck")}
              </button>
            </>
          ) : null}
          {/* "请稍后重试" must come with a way to retry. When Codex is installed
              the user can still launch it despite the failed check. */}
          {!rechecking && kind === "error" ? (
            installed ? (
              <>
                <button className="btn primary big" onClick={onLaunch} disabled={busy !== null}>
                  <CodexGlyph />
                  {t("home.launch")}
                </button>
                <button className="btn ghost" onClick={check} disabled={busy !== null}>
                  <Icon name="refresh" />
                  {t("home.recheck")}
                </button>
              </>
            ) : (
              <button className="btn primary big" onClick={check} disabled={busy !== null}>
                <Icon name="refresh" />
                {t("home.recheck")}
              </button>
            )
          ) : null}
        </div>

        {!rechecking && kind === "none" ? (
          <div className="manual-existing-entry">
            <button
              className="linkbtn subtle"
              onClick={openManualExisting}
              disabled={busy !== null || manualExistingBusy !== null}
            >
              <Icon name="folder" />
              {t("home.manualExisting")}
            </button>
          </div>
        ) : null}

        {!rechecking && kind === "update" && skippedCandidate ? (
          <div className="update-skip">
            <button className="linkbtn subtle" onClick={skipCurrentUpdate} disabled={busy !== null}>
              {t("home.skipCurrent")}
            </button>
            <span>{t("home.skipCurrent.detail", { version: skippedCandidate.version })}</span>
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

      <Sheet
        open={confirmOpen && Boolean(plan)}
        onDismiss={() => setConfirmOpen(false)}
        labelledBy={confirmTitleId}
        describedBy={confirmBodyId}
        initialFocus="primary"
      >
        <Ring icon="arrowUp" />
        <h3 id={confirmTitleId}>
          {plan ? t("confirm.title", { version: plan.latestVersion }) : ""}
        </h3>
        <p id={confirmBodyId}>
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
      </Sheet>

      <Sheet
        open={installDirOpen}
        onDismiss={() => setInstallDirOpen(false)}
        dismissable={!installDirBusy}
        labelledBy={installDirTitleId}
        describedBy={installDirBodyId}
        initialFocus="primary"
      >
        <Ring icon="download" />
        <h3 id={installDirTitleId}>{t("win.installDir.title")}</h3>
        <p id={installDirBodyId}>{t("win.installDir.body")}</p>
        <div className="sheet-path">{settings.installRoot}</div>
        <div className="row2">
          <button className="btn ghost" onClick={installToCurrentRoot} disabled={installDirBusy}>
            {t(
              installRootIsDefault ? "win.installDir.useDefault" : "win.installDir.useCurrent",
            )}
          </button>
          <button className="btn primary" onClick={browseInstallRoot} disabled={installDirBusy}>
            {t("win.installDir.browse")}
          </button>
        </div>
      </Sheet>

      <ManualExistingInstallSheet
        open={manualExistingOpen}
        candidate={manualExistingCandidate}
        error={manualExistingError}
        busy={manualExistingBusy !== null}
        labelledBy={manualExistingTitleId}
        describedBy={manualExistingBodyId}
        onDismiss={closeManualExisting}
        onPick={pickManualExisting}
        onAdopt={adoptManualExisting}
      />
    </div>
  );
}
