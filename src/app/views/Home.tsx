import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Pause, XCircle } from "lucide-react";

import {
  errorCode,
  errorMessage,
  isDownloadCancelled,
  managerApi,
} from "../../services/managerApi";
import type {
  AppSettings,
  DownloadProgress,
  MacInstallStatus,
  MacPerformReport,
  MacUpdateReport,
} from "../../shared/types";
import { DEFAULT_SETTINGS } from "../../shared/types";
import { Icon, CodexGlyph } from "../icons";
import { useI18n, dirOf, type TKey } from "../i18n";
import { Ring, TopBar, ResultBanner, ErrorHero } from "../components";
import { currentPlatform } from "../platform";
import { WinHome } from "./WinHome";
import { useCountUp } from "../useCountUp";
import { mib, fmtDateTime } from "../format";
import { useHomeMotion } from "../motion";
import { Sheet } from "../Sheet";

type Kind = "loading" | "error" | "none" | "idle" | "update" | "external" | "uptodate";
type DownloadStopIntent = "pause" | "cancel";

/** Platform dispatcher — the backend command surface differs per OS. */
export function Home(props: { onOpenSettings: () => void }) {
  return currentPlatform() === "windows" ? <WinHome {...props} /> : <MacHome {...props} />;
}

function MacHome({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { t, lang } = useI18n();
  const [report, setReport] = useState<MacUpdateReport | null>(null);
  const [status, setStatus] = useState<MacInstallStatus | null>(null);
  const [perform, setPerform] = useState<MacPerformReport | null>(null);
  // Human-facing version pair captured at perform time, so the outcome strip can
  // read "26.602.40724 → 26.602.71036" instead of raw build numbers.
  const [updatedVer, setUpdatedVer] = useState<{ from: string; to: string } | null>(null);
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [busy, setBusy] = useState<"plan" | "perform" | "adopt" | "install" | null>(null);
  const [checkError, setCheckError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  // A non-error, transient heads-up (e.g. "we re-checked because the install
  // changed"). Kept SEPARATE from `checkError` on purpose: `checkError` drives the
  // "检查失败" hero in the `!installed` branch, so reusing it for an info note
  // would strand a now-uninstalled user on an error screen with no install CTA.
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  // Whether the status request has finished (success OR failure) — distinct from
  // the value, so a failed macStatus doesn't leave the home stuck on "loading".
  const [statusLoaded, setStatusLoaded] = useState(false);
  // Whether it failed (e.g. unsupported platform) — so we don't offer install.
  const [statusFailed, setStatusFailed] = useState(false);
  // Whether a fresh install just completed — show a done state with an explicit
  // 〔打开 Codex〕 instead of auto-launching.
  const [justInstalled, setJustInstalled] = useState(false);
  // Live download progress (real bytes, emitted by the backend during
  // install/update); null when not downloading.
  const [dl, setDl] = useState<DownloadProgress | null>(null);
  const [speed, setSpeed] = useState(0);
  const dlSample = useRef<{ t: number; bytes: number } | null>(null);
  const [downloadStop, setDownloadStop] = useState<DownloadStopIntent | null>(null);
  const [downloadStopBusy, setDownloadStopBusy] = useState(false);
  const downloadStopRef = useRef<DownloadStopIntent | null>(null);
  const scopeRef = useRef<HTMLDivElement>(null);
  const confirmTitleId = useId();
  const confirmBodyId = useId();
  // Smoothly roll the live download figures instead of snapping per event.
  const dlPctTarget = dl && dl.total > 0 ? Math.min(100, (dl.downloaded / dl.total) * 100) : 0;
  const dlPct = useCountUp(dlPctTarget);
  const dlBytes = useCountUp(dl?.downloaded ?? 0);
  const dlSpeed = useCountUp(speed);

  const onDlProgress = useCallback((e: { payload: DownloadProgress }) => {
    const p = e.payload;
    setDl(p);
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
    setSpeed(0);
    dlSample.current = null;
    try {
      return await listen<DownloadProgress>("mac://download-progress", onDlProgress);
    } catch {
      // Non-Tauri (web preview): no event bus — nothing to clean up.
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
            ? await managerApi.macPauseDownload()
            : await managerApi.macCancelDownload();
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
        setActionError(errorMessage(cause));
      }
    },
    [t],
  );

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
      setCheckError(errorMessage(cause));
      return false;
    } finally {
      setBusy(null);
    }
  }, [refreshStatus]);

  useEffect(() => {
    void (async () => {
      const s = await managerApi.getSettings().catch(() => DEFAULT_SETTINGS);
      setSettings(s);
      void refreshStatus();
      // Honor "自动检查更新": only hit the appcast on open when enabled.
      if (s.autoCheck) {
        void check();
      }
    })();
  }, [check, refreshStatus]);

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

  // Window focus → silently re-detect the local install (milliseconds, no
  // network). If it no longer matches the snapshot on screen (Codex was
  // updated / downgraded out-of-band while we weren't looking), re-run the
  // full check so the card corrects itself instead of waiting to fail the
  // perform-time guard.
  useEffect(() => {
    let last = 0;
    let un: (() => void) | undefined;
    void (async () => {
      try {
        un = await listen("tauri://focus", () => {
          const now = Date.now();
          if (busyRef.current || now - last < 3000) return;
          last = now;
          void (async () => {
            try {
              const st = await managerApi.macStatus();
              setStatus(st);
              setStatusLoaded(true);
              setStatusFailed(false);
              // Re-check whenever the install identity (build OR path) differs
              // from the last checked snapshot — in EITHER direction, so a
              // fresh external install (snapshot had none), a removal, a
              // version change, and a same-build move to a new path all
              // re-plan. Gate on `checked` (we have planned at least once), not
              // on a non-null installed, or the none→installed transition is
              // missed. The perform expectation pins build+path, so a stale
              // plan here would only fail the backend guard or mislead the card.
              const checked = reportRef.current;
              const snap = checked?.installed ?? null;
              const fresh = st.installed ?? null;
              const identityChanged =
                (snap?.build ?? null) !== (fresh?.build ?? null) ||
                (snap?.path ?? null) !== (fresh?.path ?? null);
              if (checked && identityChanged) {
                // Drop any open confirm sheet first: it was built for the OLD
                // target, and the install may now be external/gone. Leaving it
                // up would let a click run perform against a snapshot the user
                // never saw — bypassing the external→adopt boundary. The user
                // re-confirms against the freshly-checked card.
                setConfirmOpen(false);
                void check();
              }
            } catch {
              // Transient/unsupported — the next explicit check will surface it.
            }
          })();
        });
      } catch {
        // Non-Tauri (web preview): no event bus — nothing to clean up.
      }
    })();
    return () => un?.();
  }, [check]);

  const adopt = useCallback(async () => {
    setBusy("adopt");
    setCheckError(null);
    setActionError(null);
    try {
      setStatus(await managerApi.macAdopt());
    } catch (cause) {
      setActionError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  }, []);

  const runInstall = useCallback(async () => {
    setBusy("install");
    setActionError(null);
    const un = await startDlListen();
    try {
      setStatus(await managerApi.macInstall());
      setJustInstalled(true);
      await check();
    } catch (cause) {
      const stop = downloadStopRef.current;
      if (stop && isDownloadCancelled(cause)) {
        setNotice(t(stop === "pause" ? "progress.paused" : "progress.cancelled"));
      } else {
        setActionError(errorMessage(cause));
      }
    } finally {
      un();
      setBusy(null);
      setDl(null);
      setDownloadStop(null);
      setDownloadStopBusy(false);
      downloadStopRef.current = null;
    }
  }, [check, startDlListen, t]);

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
      if (stop && isDownloadCancelled(cause)) {
        setNotice(t(stop === "pause" ? "progress.paused" : "progress.cancelled"));
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
        setActionError(errorMessage(cause));
      }
    } finally {
      un();
      setBusy(null);
      setDl(null);
      setDownloadStop(null);
      setDownloadStopBusy(false);
      downloadStopRef.current = null;
    }
  }, [report, check, startDlListen, t]);

  const plan = report?.plan ?? null;
  // The report is one atomic backend snapshot (installed detected together
  // with the plan) — when it exists, it is the truth the card paints and the
  // expectation signs. `status` (local-only, loads before the first network
  // check and refreshes on focus) fills in until then and drives the
  // managed/external badge.
  const installed = (report ? report.installed : status?.installed) ?? null;
  const isManaged = status?.status === "managed";
  const updateAvailable = Boolean(plan) && !plan?.upToDate;

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
    if (updateAvailable) return "update";
    return "uptodate";
  }, [busy, report, checkError, installed, updateAvailable, status, statusLoaded, statusFailed]);

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
  // Prefer the appcast's release time for the installed build; fall back to the
  // bundle's on-disk timestamp so a time is shown even when the feed omits one.
  const releaseDate = fmtDateTime(report?.installedPubDate ?? null, lang);
  const installedDate = releaseDate ? null : fmtDateTime(installed?.installedAt ?? null, lang);
  const onLaunch = () => {
    // Surface a failed open (stale path / backend error) via the error banner
    // like every other action, instead of an unhandled rejection with no feedback.
    setActionError(null);
    void managerApi.macLaunch().catch((cause) => setActionError(errorMessage(cause)));
  };

  // One string identifying the visible "scene"; when it changes the hero
  // remounts and GSAP replays the choreographed entrance (see useHomeMotion).
  // `lang` is part of the key so a language switch (Home stays mounted) remounts
  // the headline and re-splits it — otherwise SplitText's aria-label would keep
  // the old language's text for screen readers.
  const progressing = busy === "perform" || busy === "install";
  const isShimmer = progressing || rechecking || kind === "loading";
  const scene = `${lang}/${
    progressing
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

  // ── progress (performing / installing) takes over the whole screen ─────────
  if (busy === "perform" || busy === "install") {
    const known = Boolean(dl && dl.total > 0);
    const pct = known ? Math.round(dlPct) : null;
    const canStopDownload =
      Boolean(dl && dl.total > 0 && dl.downloaded < dl.total) && !downloadStopBusy;
    return (
      <div className="pop">
        <TopBar />
        <div className="scroll" ref={scopeRef}>
          <div className="hero" style={{ marginTop: 24 }} key={scene}>
            <Ring icon="loader" spin className="glow" />
            <div className="headline shimmer">
              {busy === "install" ? t("progress.installing") : t("progress.title")}
            </div>
            <div className="sub">
              {dl ? t("progress.downloadingFrom", { source: dl.source }) : t("progress.preparing")}
            </div>
            {pct !== null ? (
              <div className="pctbig">
                {pct}
                <span className="pctsign">%</span>
              </div>
            ) : null}
            <div className="bar">
              <div
                className={`bar-fill${pct === null ? " indeterminate" : ""}`}
                style={pct === null ? undefined : { width: `${dlPct}%` }}
              />
            </div>
            {known && dl ? (
              <div className="dlmeta">
                {mib(dlBytes)} / {mib(dl.total)}
                {dlSpeed > 0 ? ` · ${mib(dlSpeed)}/s` : ""}
              </div>
            ) : null}
            <div className="progress-actions">
              <button
                className="btn ghost"
                onClick={() => void requestDownloadStop("pause")}
                disabled={!canStopDownload}
              >
                <Pause />
                {downloadStop === "pause" ? t("progress.pausePending") : t("progress.pause")}
              </button>
              <button
                className="btn ghost"
                onClick={() => void requestDownloadStop("cancel")}
                disabled={!canStopDownload}
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

  // Fresh-install completion — opening Codex is the user's explicit choice.
  if (justInstalled) {
    return (
      <div className="pop">
        <TopBar />
        <div className="scroll" ref={scopeRef}>
          {actionError ? (
            <div className="banner err">
              <Icon name="alert" />
              <span>{actionError}</span>
            </div>
          ) : null}
          <section className="hero" style={{ marginTop: 16 }} key={scene}>
            <Ring icon="check" variant="success" />
            <div className="headline">{t("install.done.title")}</div>
            <div className="sub">
              {installedVersion ? t("home.uptodate.sub", { version: installedVersion }) : ""}
            </div>
          </section>
          <div className="actions">
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
        <button className="iconbtn" title={t("nav.settings")} onClick={onOpenSettings}>
          <Icon name="gear" />
        </button>
      </TopBar>

      <div className="scroll" ref={scopeRef} inert={confirmOpen ? true : undefined}>
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
        {actionError ? (
          <div className="banner err">
            <Icon name="alert" />
            <span>{actionError}</span>
          </div>
        ) : null}

        <section className="hero" key={scene}>
          {rechecking ? (
            // Mirror the settled hero's line count (ring + headline + sub +
            // status line) so nothing below shifts while the check runs.
            <>
              <Ring icon="loader" spin className="glow" />
              <div className="headline shimmer">{t("home.checking")}</div>
              <div className="sub">
                {installedVersion ? t("home.uptodate.sub", { version: installedVersion }) : " "}
              </div>
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
              <div className="sub">
                {installed ? t("home.idle.sub", { version: installedVersion }) : ""}
              </div>
              <div className="prov">
                <span className={`dot ${isManaged ? "managed" : "external"}`} />
                {isManaged ? t("prov.managed") : t("prov.external")}
              </div>
            </>
          ) : kind === "update" ? (
            <>
              <Ring icon="arrowUp" className="glow" />
              <div className="headline">{t("home.update.title")}</div>
              <div className="sub">
                <span className="ver">{latestVersion}</span>
              </div>
              <div className="flow">
                {t("home.update.flow", { from: installedVersion, to: latestVersion })}
                {plan ? ` · ${t("home.update.size", { size: mib(plan.downloadSize) })}` : ""}
              </div>
            </>
          ) : kind === "external" ? (
            <>
              <Ring icon="shield" variant="amber" />
              <div className="headline">{t("home.external.title")}</div>
              {/* Show the INSTALLED build here, not plan.latestShortVersion —
                  the latest version belongs to the update / up-to-date states. */}
              <div className="sub">
                {installed ? t("home.idle.sub", { version: installedVersion }) : ""}
              </div>
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
              <div className="sub">{t("home.uptodate.sub", { version: installedVersion })}</div>
              <div className="microcue">
                <Icon name="shield" />
                {t("home.official")} · {t("home.checkedJustNow")}
              </div>
            </>
          )}
        </section>

        {/* Installed-version details — release date (or on-disk date) + where it
            lives. Fills the calm states with genuinely useful, glanceable info. */}
        {installed && (rechecking || kind !== "loading") ? (
          <div className="list meta">
            {releaseDate ? (
              <div className="row">
                <span className="rtext">
                  <span className="rtitle">{t("home.releaseDate")}</span>
                </span>
                <span className="rval">{releaseDate}</span>
              </div>
            ) : installedDate ? (
              <div className="row">
                <span className="rtext">
                  <span className="rtitle">{t("home.installedDate")}</span>
                </span>
                <span className="rval">{installedDate}</span>
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

        <div className="actions">
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
              <button
                className="btn primary big"
                onClick={() => (settings.askBefore ? setConfirmOpen(true) : void runPerform())}
                disabled={busy !== null}
              >
                <Icon name="download" />
                {t("home.update.cta")}
              </button>
              <button className="btn ghost" onClick={onLaunch} disabled={busy !== null}>
                <CodexGlyph />
                {t("home.launch")}
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
        <div className="row2">
          <button className="btn ghost" onClick={() => setConfirmOpen(false)}>
            {t("confirm.cancel")}
          </button>
          <button className="btn primary" onClick={runPerform}>
            {t("confirm.ok")}
          </button>
        </div>
      </Sheet>
    </div>
  );
}
