import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";

import {
  errorCode,
  isDownloadCancelled,
  managerApi,
  SETTINGS_CHANGED_EVENT,
} from "../../services/managerApi";
import type {
  AppSettings,
  MacInstallStatus,
  MacPerformReport,
  MacUpdateReport,
} from "../../shared/types";
import { DEFAULT_SETTINGS } from "../../shared/types";
import { resolveFailure, userErrorMessage, type FailureSurface } from "../errorCopy";
import { Icon, CodexGlyph } from "../icons";
import { useI18n, dirOf, type TKey } from "../i18n";
import { Ring, TopBar, ResultBanner, ErrorHero, FailureBanner, StatusBanner } from "../components";
import { currentPlatform } from "../platform";
import { WinHome } from "./WinHome";
import { mib, fmtDateTime } from "../format";
import { useHomeMotion } from "../motion";
import { Sheet } from "../Sheet";
import { macSkippedUpdateCandidate, skippedUpdateMatches } from "../skippedUpdate";
import {
  ManualExistingInstallSheet,
  type ManualExistingCandidate,
} from "./ManualExistingInstall";
import { ProgressScreen, type PausedDownload } from "./ProgressScreen";
import { useDownloadProgress } from "./useDownloadProgress";
import { useFocusRecheck, installIdentity } from "./useFocusRecheck";

type Kind = "loading" | "error" | "none" | "idle" | "update" | "external" | "uptodate";

/** Platform dispatcher — the backend command surface differs per OS. */
export function Home(props: { onOpenSettings: () => void }) {
  return currentPlatform() === "windows" ? <WinHome {...props} /> : <MacHome {...props} />;
}

function MacHome({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { t, lang } = useI18n();
  const [report, setReport] = useState<MacUpdateReport | null>(null);
  const [status, setStatus] = useState<MacInstallStatus | null>(null);
  // Several coexisting Codex-lineage installs: ambient adoption refuses, the
  // explicit picker becomes the primary action (see the external button row).
  const ambiguousInstall = (status?.ambiguousPaths?.length ?? 0) > 1;
  const [perform, setPerform] = useState<MacPerformReport | null>(null);
  // Human-facing version pair captured at perform time, so the outcome strip can
  // read "26.602.40724 → 26.602.71036" instead of raw build numbers.
  const [updatedVer, setUpdatedVer] = useState<{ from: string; to: string } | null>(null);
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [busy, setBusy] = useState<"plan" | "perform" | "adopt" | "install" | null>(null);
  const [checkError, setCheckError] = useState<FailureSurface | null>(null);
  const [actionError, setActionError] = useState<FailureSurface | null>(null);
  // A non-error, transient heads-up (e.g. "we re-checked because the install
  // changed"). Kept SEPARATE from `checkError` on purpose: `checkError` drives the
  // "检查失败" hero in the `!installed` branch, so reusing it for an info note
  // would strand a now-uninstalled user on an error screen with no install CTA.
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [manualExistingOpen, setManualExistingOpen] = useState(false);
  const [manualExistingCandidate, setManualExistingCandidate] =
    useState<ManualExistingCandidate | null>(null);
  const [manualExistingBusy, setManualExistingBusy] = useState<"pick" | "adopt" | null>(null);
  const [manualExistingError, setManualExistingError] = useState<string | null>(null);
  // Whether the status request has finished (success OR failure) — distinct from
  // the value, so a failed macStatus doesn't leave the home stuck on "loading".
  const [statusLoaded, setStatusLoaded] = useState(false);
  // Whether it failed (e.g. unsupported platform) — so we don't offer install.
  const [statusFailed, setStatusFailed] = useState(false);
  // Whether a fresh install just completed — show a done state with an explicit
  // 〔打开 Codex〕 instead of auto-launching.
  const [justInstalled, setJustInstalled] = useState(false);
  // A paused download: the progress screen stays up (not routed home) offering
  // 〔继续〕/〔取消〕. `dl` is the byte snapshot captured at the moment of pause.
  const [paused, setPaused] = useState<PausedDownload | null>(null);
  const scopeRef = useRef<HTMLDivElement>(null);
  const confirmTitleId = useId();
  const confirmBodyId = useId();
  const manualExistingTitleId = useId();
  const manualExistingBodyId = useId();

  // Live download state machine (bytes + eased readouts + pause/cancel intent),
  // shared with the Windows home; only the channel + stop commands differ.
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
    requestDownloadStop,
    resetStop,
  } = useDownloadProgress({
    eventName: "mac://download-progress",
    pauseDownload: () => managerApi.macPauseDownload(),
    cancelDownload: () => managerApi.macCancelDownload(),
    cannotCancelMessage: t("progress.cannotCancel"),
    onError: setActionError,
  });

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await managerApi.macStatus());
      setStatusFailed(false);
    } catch {
      // e.g. unsupported platform — record the failure so we don't offer install.
      setStatusFailed(true);
    } finally {
      setStatusLoaded(true);
    }
  }, []);

  // Re-plan AND re-detect in the same breath: everything on screen (and the
  // perform expectation) must come from one coherent moment. Refreshing only
  // one of the two is how "当前 X → 新版 X" cards and doomed expectations happen
  // after an out-of-band install change. Returns whether the check succeeded,
  // so a caller can layer a notice on top without masking a check failure.
  const check = useCallback(async (): Promise<boolean> => {
    setBusy("plan");
    setCheckError(null);
    setActionError(null);
    setNotice(null);
    try {
      const [r] = await Promise.all([managerApi.macPlanUpdate(), refreshStatus()]);
      setReport(r);
      return true;
    } catch (cause) {
      // Drop any stale plan so a failed re-check can't keep driving "立即更新"
      // off an outdated currentBuild/latestBuild.
      setReport(null);
      setCheckError(resolveFailure(cause, t));
      return false;
    } finally {
      setBusy(null);
    }
  }, [refreshStatus, t]);

  useEffect(() => {
    void (async () => {
      const s = await managerApi.getSettings().catch(() => DEFAULT_SETTINGS);
      setSettings(s);
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

  // The snapshot/busy values the focus listener (a long-lived subscription)
  // reads — refs so the subscription doesn't tear down on every state change.
  const reportRef = useRef<MacUpdateReport | null>(null);
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

  useEffect(() => {
    if (!settings.periodicCheck) return;
    const intervalMs = Math.max(60_000, settings.periodicCheckIntervalSeconds * 1000);
    const id = window.setInterval(() => {
      if (busyRef.current) return;
      void checkRef.current();
    }, intervalMs);
    return () => window.clearInterval(id);
  }, [settings.periodicCheck, settings.periodicCheckIntervalSeconds]);

  // Re-check whenever the install identity (build OR path) differs from the
  // last checked snapshot — in EITHER direction, so a fresh external install
  // (snapshot had none), a removal, a version change, and a same-build move to
  // a new path all re-plan. The perform expectation pins build+path, so a stale
  // plan would only fail the backend guard or mislead the card. Dropping any
  // open confirm sheet first: it was built for the OLD target.
  useFocusRecheck<MacInstallStatus>({
    fetchStatus: () => managerApi.macStatus(),
    onStatus: (st) => {
      setStatus(st);
      setStatusLoaded(true);
      setStatusFailed(false);
    },
    hasChecked: () => reportRef.current != null,
    checkedIdentity: () => installIdentity(reportRef.current?.installed ?? null),
    identityOf: (st) => installIdentity(st.installed ?? null),
    isBusy: () => busyRef.current != null,
    onIdentityChanged: () => {
      setConfirmOpen(false);
      void check();
    },
  });

  const adopt = useCallback(async () => {
    setBusy("adopt");
    setCheckError(null);
    setActionError(null);
    try {
      setStatus(await managerApi.macAdopt());
    } catch (cause) {
      setActionError(resolveFailure(cause, t));
    } finally {
      setBusy(null);
    }
  }, [t]);

  const runInstall = useCallback(async () => {
    setBusy("install");
    setActionError(null);
    setPaused(null);
    const un = await startDlListen();
    try {
      const status = await managerApi.macInstall();
      setStatus(status);
      // Primary install can succeed while provenance fails — still a completed
      // install on disk; never treat it as a hard failure (would invite reinstall).
      const outcome = status.outcome;
      if (outcome?.primaryOk && outcome.recoveryActions.includes("record_provenance")) {
        setNotice(t("install.partial.note"));
        setJustInstalled(true);
      } else {
        setJustInstalled(true);
      }
      await check();
    } catch (cause) {
      const stop = downloadStopRef.current;
      if (stop === "pause" && isDownloadCancelled(cause)) {
        // Keep the progress screen up in a paused state instead of routing home;
        // the cached `.part` survives, so 〔继续〕 resumes from here.
        setPaused({ kind: "install", dl: dlRef.current });
      } else if (stop && isDownloadCancelled(cause)) {
        setNotice(t("progress.cancelled"));
      } else {
        setActionError(resolveFailure(cause, t));
      }
    } finally {
      un();
      setBusy(null);
      resetStop();
    }
  }, [check, startDlListen, resetStop, dlRef, downloadStopRef, t]);

  const runPerform = useCallback(async () => {
    // ONE atomic snapshot (the report carries installed + plan detected
    // together) drives both the labels and the consent expectation — never mix
    // it with the separately-fetched `status`, which can lag an out-of-band
    // install change. The backend re-verifies reality against exactly this
    // expectation before the destructive swap.
    const installed = report?.installed;
    const plan = report?.plan;
    if (!installed || !plan || plan.upToDate) return;
    setBusy("perform");
    setActionError(null);
    setPaused(null);
    // Capture the human-facing versions BEFORE the swap — afterward a re-check
    // makes installed/latest identical. Fall back to a build number only if a
    // feed omits the short version.
    const fromVersion = installed.shortVersion || `build ${installed.build}`;
    const toVersion = plan.latestShortVersion || `build ${plan.latestBuild}`;
    const un = await startDlListen();
    try {
      const result = await managerApi.macPerformUpdate({
        fromBuild: installed.build,
        toBuild: plan.latestBuild,
        path: installed.path,
      });
      setPerform(result);
      setUpdatedVer({ from: fromVersion, to: toVersion });
      setConfirmOpen(false);
      await check();
    } catch (cause) {
      setConfirmOpen(false);
      const stop = downloadStopRef.current;
      if (stop === "pause" && isDownloadCancelled(cause)) {
        // Stay on the progress screen as paused; the cached `.part` lets 〔继续〕
        // resume from here instead of restarting at 0.
        setPaused({ kind: "perform", dl: dlRef.current });
      } else if (stop && isDownloadCancelled(cause)) {
        setNotice(t("progress.cancelled"));
      } else if (errorCode(cause) === "stale_expectation") {
        // Reality moved between confirm and execute (the backend's TOCTOU
        // guard). Refresh the snapshot and post a NOTICE (not an error) so the
        // card can settle into whatever it now is — update / up-to-date / none
        // (Codex was removed) — without the `checkError`-driven "检查失败" hero
        // hijacking the none case. A failed re-check keeps its own error.
        if (await check()) {
          setNotice(t("home.stale.rechecked"));
        }
      } else {
        setActionError(resolveFailure(cause, t));
      }
    } finally {
      un();
      setBusy(null);
      resetStop();
    }
  }, [report, check, startDlListen, resetStop, dlRef, downloadStopRef, t]);

  // 〔继续〕from the paused state — re-run the same operation. The backend finds
  // the cached `.part` and resumes via `curl -C -`, so the bar picks up where it
  // stopped instead of at 0.
  const resumeDownload = useCallback(() => {
    const kind = paused?.kind;
    setPaused(null);
    if (kind === "install") void runInstall();
    else void runPerform();
  }, [paused, runInstall, runPerform]);

  // 〔取消〕from the paused state — the download already stopped, so drop the
  // cached partial and route home. (An in-flight cancel is handled by
  // requestDownloadStop instead.)
  const cancelPausedDownload = useCallback(async () => {
    setPaused(null);
    try {
      // Only claim "已取消" once the cached partial is actually gone — otherwise
      // a failed discard would leave a `.part` that the next update silently
      // resumes, contradicting the cancel.
      await managerApi.macDiscardDownload();
      setNotice(t("progress.cancelled"));
    } catch (cause) {
      setActionError(resolveFailure(cause, t));
    }
  }, [t]);

  const plan = report?.plan ?? null;
  // The report is one atomic backend snapshot (installed detected together
  // with the plan) — when it exists, it is the truth the card paints and the
  // expectation signs. `status` (local-only, loads before the first network
  // check and refreshes on focus) fills in until then and drives the
  // managed/external badge.
  const installed = (report ? report.installed : status?.installed) ?? null;
  const isManaged = status?.status === "managed";
  const skippedCandidate = useMemo(() => macSkippedUpdateCandidate(report), [report]);
  const updateSuppressed = skippedUpdateMatches(settings.skippedCodexUpdate, skippedCandidate);
  const updateAvailable = Boolean(plan) && !plan?.upToDate && !updateSuppressed;

  const kind: Kind = useMemo(() => {
    if (!installed) {
      if (busy === "plan" || !statusLoaded) return "loading";
      // A failed status (unsupported platform) OR a failed check (OS too old /
      // appcast unreachable) → error, never an install entry that would just
      // fail again.
      if (statusFailed || checkError) return "error";
      return "none";
    }
    // Don't classify (update / idle / uptodate) until the local adoption status
    // is known — otherwise report.installed can drive an "update" entry that
    // bypasses the external→adopt boundary once status resolves to external.
    if (!statusLoaded) return "loading";
    // Adoption status is local (macStatus) and must not depend on a successful
    // network check — surface "开始管理" before any network-dependent state so it
    // is never hidden (auto-check off / appcast error) or bypassed (update).
    if (status?.status === "external") return "external";
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
    statusLoaded,
    statusFailed,
  ]);

  // Always show the LOCAL installed version (CFBundleShortVersionString), never
  // the source's latest — they differ when the mirror lags or Codex was updated
  // out-of-band. `latestVersion` is the update target, shown only when updating.
  const installedVersion =
    installed?.shortVersion || (installed ? `build ${installed.build}` : "");
  const latestVersion = plan?.latestShortVersion || "";
  const sourceLabel = t(`source.${settings.source}` as TKey);

  // A re-check (or the first auto-check) while an app is already known: the hero
  // morphs to the checking state so "已是最新" visibly reacts, then settles back.
  const rechecking = busy === "plan" && Boolean(installed);
  // Prefer the appcast's release time for the installed build. If the feed
  // omits it, skip the date row rather than showing an install timestamp.
  const releaseDate = fmtDateTime(report?.installedPubDate ?? null, lang);
  const latestReleaseDate = updateAvailable
    ? fmtDateTime(report?.latestPubDate ?? null, lang)
    : null;
  const updateSize =
    updateAvailable && plan ? t("home.update.size", { size: mib(plan.downloadSize) }) : null;

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
      const selected = await managerApi.macPickExistingInstall();
      if (selected) {
        setManualExistingCandidate({
          path: selected.path,
          version: selected.shortVersion || `build ${selected.build}`,
          releaseDate: null,
        });
      }
    } catch (cause) {
      setManualExistingError(userErrorMessage(cause, t));
    } finally {
      setManualExistingBusy(null);
    }
  }, [t]);

  const adoptManualExisting = useCallback(async () => {
    if (!manualExistingCandidate) return;
    setManualExistingBusy("adopt");
    setManualExistingError(null);
    try {
      const next = await managerApi.macAdoptPath(manualExistingCandidate.path);
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
    // Surface a failed open (stale path / backend error) via the error banner
    // like every other action, instead of an unhandled rejection with no feedback.
    setActionError(null);
    void managerApi.macLaunch().catch((cause) => setActionError(resolveFailure(cause, t)));
  };

  // One string identifying the visible "scene"; when it changes the hero
  // remounts and GSAP replays the choreographed entrance (see useHomeMotion).
  // `lang` is part of the key so a language switch (Home stays mounted) remounts
  // the headline and re-splits it — otherwise SplitText's aria-label would keep
  // the old language's text for screen readers.
  const progressing = busy === "perform" || busy === "install";
  // The paused screen is calm (no shimmer): the headline is a settled "已暂停",
  // not an in-flight state.
  const isShimmer = progressing || rechecking || kind === "loading";
  const scene = `${lang}/${
    paused
      ? `paused-${paused.kind}`
      : progressing
        ? `progress-${busy}`
        : justInstalled
          ? "done"
          : `${kind}${rechecking ? "-checking" : ""}`
  }`;
  const success = justInstalled || (!rechecking && kind === "uptodate");
  // Outcome-strip detail + persistence. Surface a relaunch-failure prompt and
  // any backend warning (provenance save failure, kept backup path), and pin the
  // strip — no auto-dismiss — whenever there's something to act on or read, so a
  // real warning never silently fades. A fully clean update has neither and
  // self-dismisses.
  const performDetail =
    (perform &&
      [perform.relaunchFailed ? t("success.manualLaunch") : null, perform.warning]
        .filter(Boolean)
        .join(" · ")) ||
    undefined;
  const performPinned = Boolean(
    perform && (perform.rolledBack || perform.relaunchFailed || perform.warning),
  );
  // Char-split only LTR scripts — splitting a cursive RTL script (Arabic) into
  // per-char elements breaks its contextual letter joining.
  const splitHeadline = !isShimmer && dirOf(lang) === "ltr";
  useHomeMotion(scopeRef, scene, { splitHeadline, success });

  // ── progress (performing / installing / paused) takes over the whole screen ─
  if (busy === "perform" || busy === "install" || paused) {
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

  // Fresh-install completion — opening Codex is the user's explicit choice.
  if (justInstalled) {
    const needsRecord =
      status?.outcome?.recoveryActions.includes("record_provenance") ||
      status?.status === "external";
    return (
      <div className="pop">
        <TopBar />
        <div className="scroll" ref={scopeRef}>
          {actionError ? <FailureBanner failure={actionError} /> : null}
          {notice || needsRecord ? (
            <StatusBanner tone="warn">{notice ?? t("install.partial.note")}</StatusBanner>
          ) : null}
          <section className="hero" style={{ marginTop: 16 }} key={scene}>
            <Ring icon="check" variant="success" />
            <div className="headline">{t("install.done.title")}</div>
            <div className="sub">
              {installedVersion ? t("home.uptodate.sub", { version: installedVersion }) : ""}
            </div>
          </section>
          <div className="actions">
            {needsRecord ? (
              <button
                className="btn primary big"
                onClick={() => {
                  setJustInstalled(false);
                  void adopt();
                }}
                disabled={busy === "adopt"}
              >
                {t("install.partial.record")}
              </button>
            ) : null}
            <button className="btn primary big" onClick={onLaunch}>
              <CodexGlyph />
              {t("install.done.open")}
            </button>
            <button className="btn ghost" onClick={() => setJustInstalled(false)}>
              {t("success.done")}
            </button>
          </div>
        </div>
      </div>
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
        inert={confirmOpen || manualExistingOpen ? true : undefined}
      >
        {perform ? (
          <ResultBanner
            tone={perform.rolledBack ? "err" : "ok"}
            title={
              perform.rolledBack
                ? t("success.rolledBack")
                : updatedVer
                  ? t("success.flow", { from: updatedVer.from, to: updatedVer.to })
                  : t("success.title")
            }
            detail={perform.rolledBack ? undefined : performDetail}
            autoDismissMs={performPinned ? undefined : 6000}
            onClose={() => {
              setPerform(null);
              setUpdatedVer(null);
            }}
          />
        ) : null}
        {notice ? (
          <ResultBanner
            tone="ok"
            title={notice}
            autoDismissMs={6000}
            onClose={() => setNotice(null)}
          />
        ) : null}
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
              {/* Paths are technical content, shown verbatim in every locale. */}
              {ambiguousInstall && status?.ambiguousPaths ? (
                <div className="desc mono">{status.ambiguousPaths.join(" · ")}</div>
              ) : null}
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
            {updateAvailable && latestVersion ? (
              <div className="row update-meta">
                <span className="rtext">
                  <span className="rtitle">{t("home.update.title")}</span>
                </span>
                <span className="rval version latest">{latestVersion}</span>
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
            {installedVersion ? (
              <div className="row">
                <span className="rtext">
                  <span className="rtitle">{t("home.currentVersion")}</span>
                </span>
                <span className="rval version">{installedVersion}</span>
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

        <div className={`actions${!rechecking && kind === "update" ? " update-actions" : ""}`}>
          {/* While a check runs we keep a STABLE pair of buttons (launch +
              the spinning re-check) so nothing reflows under the hero. */}
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
                onClick={() => (settings.askBefore ? setConfirmOpen(true) : void runPerform())}
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
              {/* With several coexisting Codex installs, ambient adoption
                  would refuse (and must not silently pick one) — make the
                  explicit picker the primary action instead. */}
              {ambiguousInstall ? (
                <button
                  className="btn primary big"
                  onClick={openManualExisting}
                  disabled={busy !== null || manualExistingBusy !== null}
                >
                  <Icon name="folder" />
                  {t("home.manualExisting.title")}
                </button>
              ) : (
                <button className="btn primary big" onClick={adopt} disabled={busy !== null}>
                  <Icon name="shield" />
                  {t("home.external.cta")}
                </button>
              )}
              <button className="btn ghost" onClick={onLaunch} disabled={busy !== null}>
                <CodexGlyph />
                {t("home.launch")}
              </button>
            </>
          ) : null}
          {!rechecking && kind === "none" ? (
            <button className="btn primary big" onClick={runInstall} disabled={busy !== null}>
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

        {/* Also offered on non-ambiguous "external" (on ambiguity the picker
            IS the primary action above, so the secondary link would repeat). */}
        {!rechecking && (kind === "none" || (kind === "external" && !ambiguousInstall)) ? (
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
        <Ring icon="arrowUp" className="" />
        <h3 id={confirmTitleId}>{t("confirm.title", { version: latestVersion })}</h3>
        <p id={confirmBodyId}>{t("confirm.body")}</p>
        <div className="row2 sheet-actions">
          <button className="btn ghost" onClick={() => setConfirmOpen(false)}>
            {t("confirm.cancel")}
          </button>
          <button className="btn primary" onClick={runPerform}>
            {t("confirm.ok")}
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
