import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";

import { errorCode, errorMessage, managerApi } from "../../services/managerApi";
import type {
  AppSettings,
  DownloadProgress,
  WinInstallStatus,
  WinPerformReport,
  WinUpdateReport,
} from "../../shared/types";
import { DEFAULT_SETTINGS } from "../../shared/types";
import { Icon, CodexGlyph } from "../icons";
import { useI18n, dirOf, type TKey } from "../i18n";
import { Ring, TopBar, ResultBanner, ErrorHero } from "../components";
import { useCountUp } from "../useCountUp";
import { mib, fmtDateTime } from "../format";
import { useHomeMotion } from "../motion";

function samePath(a: string, b: string): boolean {
  const norm = (value: string) =>
    value.trim().replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();
  return norm(a) === norm(b);
}

type Kind = "loading" | "error" | "none" | "idle" | "update" | "external" | "uptodate";

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
  const [statusLoaded, setStatusLoaded] = useState(false);
  const [statusFailed, setStatusFailed] = useState(false);
  const [dl, setDl] = useState<DownloadProgress | null>(null);
  const [speed, setSpeed] = useState(0);
  const dlSample = useRef<{ t: number; bytes: number } | null>(null);
  const scopeRef = useRef<HTMLDivElement>(null);
  // Smoothly roll the live download figures instead of snapping per event.
  const dlPctTarget = dl && dl.total > 0 ? Math.min(100, (dl.downloaded / dl.total) * 100) : 0;
  const dlPct = useCountUp(dlPctTarget);
  const dlBytes = useCountUp(dl?.downloaded ?? 0);
  const dlSpeed = useCountUp(speed);

  const onDlProgress = useCallback((event: { payload: DownloadProgress }) => {
    const p = event.payload;
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
      return await listen<DownloadProgress>("win://download-progress", onDlProgress);
    } catch {
      return () => {};
    }
  }, [onDlProgress]);

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
      if (s.autoCheck) {
        void check();
      }
    })();
  }, [check, refreshStatus]);

  const adopt = useCallback(async () => {
    setBusy("adopt");
    setCheckError(null);
    setActionError(null);
    setNotice(null);
    try {
      setStatus(await managerApi.winAdopt());
    } catch (cause) {
      setActionError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  }, []);

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
      setActionError(errorMessage(cause));
      return;
    }
    await check();
  }, [settings, check]);

  // Windows install + update both go through win_perform_update (the route —
  // MSIX sideload or portable fallback — is decided by the backend plan).
  const runPerform = useCallback(
    async (mode: "perform" | "install", installRoot?: string) => {
      setBusy(mode);
      setActionError(null);
      setNotice(null);
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
        if (errorCode(cause) === "stale_expectation") {
          await refreshStatus();
          if (await check()) {
            setNotice(t("home.stale.rechecked"));
          }
        } else {
          setActionError(errorMessage(cause));
        }
      } finally {
        unlisten();
        setBusy(null);
        setDl(null);
      }
    },
    [status, report, refreshStatus, check, startDlListen, t],
  );

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
      setActionError(errorMessage(cause));
      setInstallDirOpen(false);
    } finally {
      setInstallDirBusy(false);
    }
  }, [runPerform]);

  const plan = report?.plan ?? null;
  const installed = report ? report.installed : status?.installed ?? null;
  const statusMatchesInstalled = Boolean(
    installed &&
      status?.installed &&
      samePath(installed.path, status.installed.path) &&
      installed.version === status.installed.version,
  );
  const isManaged = statusMatchesInstalled && status?.status === "managed";
  const updateAvailable = Boolean(plan) && !plan?.upToDate;
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
    if (updateAvailable) return "update";
    return "uptodate";
  }, [
    busy,
    report,
    checkError,
    installed,
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
  // Windows has no Sparkle feed, so the date is the on-disk install time.
  const installedDate = fmtDateTime(installed?.installedAt ?? null, lang);
  const onLaunch = () => {
    // Surface a failed open (PowerShell/AUMID or portable-exe error) via the
    // error banner like every other action, not an unhandled rejection.
    setActionError(null);
    void managerApi.winLaunch().catch((cause) => setActionError(errorMessage(cause)));
  };

  // Scene id; on change the hero remounts and GSAP replays the entrance. `lang`
  // is part of the key so a language switch re-splits the headline (otherwise
  // SplitText's aria-label keeps the old language's text for screen readers).
  const progressing = busy === "perform" || busy === "install";
  const isShimmer = progressing || rechecking || kind === "loading";
  const scene = `${lang}/${progressing ? `progress-${busy}` : `${kind}${rechecking ? "-checking" : ""}`}`;
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

  if (progressing) {
    const known = Boolean(dl && dl.total > 0);
    const pct = known ? Math.round(dlPct) : null;
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

      <div className="scroll" ref={scopeRef}>
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
            // Mirror the settled hero's line count (ring + headline + sub +
            // status line) so nothing below shifts while the check runs.
            <>
              <Ring icon="loader" spin className="glow" />
              <div className="headline shimmer">{t("home.checking")}</div>
              <div className="sub">
                {version ? t("home.uptodate.sub", { version }) : " "}
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
              <div className="sub">{t("win.installSub", { version })}</div>
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
              <Ring icon="check" variant="success" />
              <div className="headline">{t("home.uptodate.title")}</div>
              <div className="sub">{t("home.uptodate.sub", { version })}</div>
              <div className="microcue">
                <Icon name="shield" />
                {t("home.official")} · {t("home.checkedJustNow")}
              </div>
            </>
          )}
        </section>

        {/* Installed-version details — on-disk install time + where it lives. */}
        {installed && (rechecking || kind !== "loading") ? (
          <div className="list meta">
            {installedDate ? (
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

        {!rechecking && msixRisky && (kind === "none" || kind === "update") ? (
          <div className="banner warn">
            <Icon name="alert" />
            <span>{t("win.msixRisk.body")}</span>
            <button className="linkbtn" onClick={switchToPortable} disabled={busy !== null}>
              {t("win.msixRisk.switch")}
            </button>
          </div>
        ) : null}

        <div className="actions">
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
              <button
                className="btn primary big"
                onClick={() => (settings.askBefore ? setConfirmOpen(true) : void runPerform("perform"))}
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

      {installDirOpen ? (
        <div className="scrim" onClick={() => (installDirBusy ? undefined : setInstallDirOpen(false))}>
          <div className="sheet" onClick={(e) => e.stopPropagation()}>
            <Ring icon="download" />
            <h3>{t("win.installDir.title")}</h3>
            <p>{t("win.installDir.body")}</p>
            <div className="sheet-path">{settings.installRoot}</div>
            <div className="row2">
              <button
                className="btn ghost"
                onClick={installToCurrentRoot}
                disabled={installDirBusy}
              >
                {t(
                  installRootIsDefault
                    ? "win.installDir.useDefault"
                    : "win.installDir.useCurrent",
                )}
              </button>
              <button
                className="btn primary"
                onClick={browseInstallRoot}
                disabled={installDirBusy}
              >
                {t("win.installDir.browse")}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
