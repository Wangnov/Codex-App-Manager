import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

import {
  errorCode,
  isDownloadCancelled,
  managerApi,
  SETTINGS_CHANGED_EVENT,
} from "../../services/managerApi";
import type {
  AppSettings,
  WinInstallStatus,
  WinPerformReport,
  WinUpdateReport,
} from "../../shared/types";
import { DEFAULT_SETTINGS } from "../../shared/types";
import { resolveFailure, userErrorMessage, type FailureSurface } from "../errorCopy";
import { Icon, CodexGlyph } from "../icons";
import { useI18n, dirOf, type TKey } from "../i18n";
import { Ring, TopBar, ResultBanner, ErrorHero, FailureBanner, StatusBanner } from "../components";
import { mib, fmtDateTime } from "../format";
import { samePath, normalizePath } from "../paths";
import { useHomeMotion } from "../motion";
import { Sheet } from "../Sheet";
import { skippedUpdateMatches, winSkippedUpdateCandidate } from "../skippedUpdate";
import {
  ManualExistingInstallSheet,
  type ManualExistingCandidate,
} from "./ManualExistingInstall";
import { ProgressScreen, type PausedDownload } from "./ProgressScreen";
import { useDownloadProgress } from "./useDownloadProgress";
import { useFocusRecheck, installIdentity } from "./useFocusRecheck";
import { useOperationReattach } from "./useOperationReattach";

type Kind = "loading" | "error" | "none" | "idle" | "update" | "external" | "uptodate";

// Windows counterpart of MacHome — same design system + state machine, driven by
// the win_* backend (codex-win-engine): MSIX sideload or portable fallback.
export function WinHome({
  onOpenSettings,
  managerUpdateSlot,
}: {
  onOpenSettings: () => void;
  managerUpdateSlot?: ReactNode;
}) {
  const { t, lang } = useI18n();
  const [report, setReport] = useState<WinUpdateReport | null>(null);
  const [status, setStatus] = useState<WinInstallStatus | null>(null);
  const [perform, setPerform] = useState<WinPerformReport | null>(null);
  // Version pair captured at update time (fresh installs have no "from"), so the
  // outcome strip can read "X → Y" like the mac side.
  const [updatedVer, setUpdatedVer] = useState<{ from: string; to: string } | null>(null);
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [defaultInstallRoot, setDefaultInstallRoot] = useState(DEFAULT_SETTINGS.installRoot);
  const [busy, setBusy] = useState<"plan" | "perform" | "adopt" | "install" | "launch" | null>(
    null,
  );
  const [checkError, setCheckError] = useState<FailureSurface | null>(null);
  const [actionError, setActionError] = useState<FailureSurface | null>(null);
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
  // A paused download: the progress screen stays up (not routed home) offering
  // 〔继续〕/〔取消〕. `installRoot` is preserved so a paused fresh install
  // resumes into the same chosen location.
  const [paused, setPaused] = useState<(PausedDownload & { installRoot?: string }) | null>(null);
  const scopeRef = useRef<HTMLDivElement>(null);
  // Synchronous guard for launch double-clicks (state alone can miss a second
  // click before setBusy("launch") re-renders).
  const launchInFlightRef = useRef(false);
  const confirmTitleId = useId();
  const confirmBodyId = useId();
  const installDirTitleId = useId();
  const installDirBodyId = useId();
  const manualExistingTitleId = useId();
  const manualExistingBodyId = useId();

  // Live download state machine, shared with the mac home; only the channel +
  // stop commands differ.
  const {
    dl,
    dlRef,
    dlPct,
    dlBytes,
    dlSpeed,
    downloadStop,
    downloadStopBusy,
    downloadStopRef,
    startDlListen,
    applySnapshotProgress,
    requestDownloadStop,
    resetStop,
  } = useDownloadProgress({
    eventName: "win://download-progress",
    pauseDownload: () => managerApi.winPauseDownload(),
    cancelDownload: () => managerApi.winCancelDownload(),
    cannotCancelMessage: t("progress.cannotCancel"),
    onError: setActionError,
  });

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
      setCheckError(resolveFailure(cause, t));
      return false;
    } finally {
      setBusy(null);
    }
  }, [t]);

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
      // Skip the startup check when an install/update is already mid-flight —
      // reattach owns the screen until that lease ends.
      const snap = await Promise.resolve()
        .then(() => managerApi.getOperationSnapshot())
        .catch(() => null);
      if (snap && (snap.kind === "install" || snap.kind === "update")) {
        return;
      }
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

  // The snapshot/busy values the focus listener (a long-lived subscription)
  // reads — refs so the subscription doesn't tear down on every state change.
  const reportRef = useRef<WinUpdateReport | null>(null);
  useEffect(() => {
    reportRef.current = report;
  }, [report]);
  const busyRef = useRef(busy);
  useEffect(() => {
    busyRef.current = busy;
  }, [busy]);
  const checkRef = useRef(check);
  useEffect(() => {
    checkRef.current = check;
  }, [check]);

  // After renderer reload: query OperationSnapshot, rebuild progress listeners
  // by operation id, and poll until the backend lease ends.
  useOperationReattach({
    startDlListen,
    applySnapshotProgress,
    resetStop,
    setBusy: (next) => setBusy(next),
    setPaused,
    onOperationEnded: () => {
      void checkRef.current();
      void refreshStatus();
    },
    isLocallyBusy: () => {
      const b = busyRef.current;
      return b === "perform" || b === "install";
    },
  });

  // Window focus → silently re-detect the local install and re-check if the
  // install identity (version OR path) drifted out-of-band. Parity with the mac
  // home, which has had this since the atomic-snapshot rework; without it the
  // Windows card can show a stale version / "managed" badge after Codex is
  // updated or removed externally, until the next periodic check.
  useFocusRecheck<WinInstallStatus>({
    fetchStatus: () => managerApi.winStatus(),
    onStatus: (st) => {
      setStatus(st);
      setStatusLoaded(true);
      setStatusFailed(false);
    },
    hasChecked: () => reportRef.current != null,
    checkedIdentity: () => installIdentity(reportRef.current?.installed ?? null, normalizePath),
    identityOf: (st) => installIdentity(st.installed ?? null, normalizePath),
    isBusy: () => busyRef.current != null,
    onIdentityChanged: () => {
      // Drop EVERY sheet built for the OLD target before re-checking: the
      // confirm sheet, the fresh-install location sheet, and the manual
      // existing-install picker. Any of them could otherwise let a click run
      // install/perform against a snapshot the user never saw — bypassing the
      // freshly-refreshed external→adopt boundary. The user re-confirms against
      // the re-checked card.
      setConfirmOpen(false);
      setInstallDirOpen(false);
      setManualExistingOpen(false);
      setManualExistingCandidate(null);
      void check();
    },
  });

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
      setActionError(resolveFailure(cause, t));
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
      setActionError(resolveFailure(cause, t));
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
        // Partial success (app installed, provenance failed): keep success path
        // and surface a recovery notice — never treat as hard failure.
        if (
          result.success &&
          result.outcome?.recoveryActions?.includes("record_provenance")
        ) {
          setNotice(t("install.partial.note"));
        }
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
          setActionError(resolveFailure(cause, t));
        }
      } finally {
        unlisten();
        setBusy(null);
        resetStop();
      }
    },
    [status, report, refreshStatus, check, startDlListen, resetStop, dlRef, downloadStopRef, t],
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
      setActionError(resolveFailure(cause, t));
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
      setCheckError(resolveFailure(cause, t));
      return null;
    } finally {
      setBusy(null);
    }
  }, [report?.plan?.route, settings.windowsInstallMode, t]);

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
      setActionError(resolveFailure(cause, t));
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
      setManualExistingError(userErrorMessage(cause, t));
    } finally {
      setManualExistingBusy(null);
    }
  }, [releaseDate, t]);

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
      setManualExistingError(userErrorMessage(cause, t));
    } finally {
      setManualExistingBusy(null);
    }
  }, [check, manualExistingCandidate, t]);

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
      setActionError(resolveFailure(cause, t));
    }
  }, [settings, skippedCandidate, t]);
  const onLaunch = () => {
    if (busy !== null || launchInFlightRef.current) return;
    // Surface a failed open (PowerShell/AUMID or portable-exe error) via the
    // error banner like every other action, not an unhandled rejection.
    launchInFlightRef.current = true;
    setActionError(null);
    setBusy("launch");
    void managerApi
      .winLaunch()
      .catch((cause) => setActionError(resolveFailure(cause, t)))
      .finally(() => {
        launchInFlightRef.current = false;
        setBusy(null);
      });
  };
  const launching = busy === "launch";
  const launchButton = (variant: "primary" | "ghost") => (
    <button
      className={variant === "primary" ? "btn primary big" : "btn ghost"}
      onClick={onLaunch}
      disabled={busy !== null}
      aria-busy={launching}
    >
      {launching ? <Icon name="loader" className="spinicon" /> : <CodexGlyph />}
      {launching ? t("home.launching") : t("home.launch")}
    </button>
  );

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
    return (
      <ProgressScreen
        scene={scene}
        scopeRef={scopeRef}
        paused={paused}
        dl={dl}
        dlPct={dlPct}
        dlBytes={dlBytes}
        dlSpeed={dlSpeed}
        installing={paused ? paused.kind === "install" : busy === "install"}
        downloadStop={downloadStop}
        downloadStopBusy={downloadStopBusy}
        onResume={resumeDownload}
        onPause={() => void requestDownloadStop("pause")}
        onCancel={() => {
          if (paused) void cancelPausedDownload();
          else void requestDownloadStop("cancel");
        }}
      />
    );
  }

  return (
    <div className="pop">
      <TopBar>
        <button
          className="iconbtn"
          data-page-focus
          title={t("nav.settings")}
          onClick={onOpenSettings}
        >
          <Icon name="gear" />
        </button>
      </TopBar>

      <div
        className="scroll"
        ref={scopeRef}
        inert={confirmOpen || installDirOpen || manualExistingOpen ? true : undefined}
      >
        {managerUpdateSlot}
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
        {notice ? <StatusBanner tone="info">{notice}</StatusBanner> : null}
        {actionError ? <FailureBanner failure={actionError} /> : null}

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
            <ErrorHero failure={checkError} />
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
              {launchButton("primary")}
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
              {launchButton("primary")}
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
              {launchButton("ghost")}
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
              {launchButton("primary")}
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
                {launchButton("primary")}
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
        <div className="row2 sheet-actions">
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
        <div className="row2 sheet-actions">
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
