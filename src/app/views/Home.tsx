import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";

import { errorMessage, managerApi } from "../../services/managerApi";
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
import { Ring, TopBar } from "../components";
import { currentPlatform } from "../platform";
import { WinHome } from "./WinHome";
import { useCountUp } from "../useCountUp";
import { mib, fmtDateTime } from "../format";
import { useHomeMotion } from "../motion";

type Kind = "loading" | "error" | "none" | "idle" | "update" | "external" | "uptodate";

/** Platform dispatcher — the backend command surface differs per OS. */
export function Home(props: { onOpenSettings: () => void }) {
  return currentPlatform() === "windows" ? <WinHome {...props} /> : <MacHome {...props} />;
}

function MacHome({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { t, lang } = useI18n();
  const [report, setReport] = useState<MacUpdateReport | null>(null);
  const [status, setStatus] = useState<MacInstallStatus | null>(null);
  const [perform, setPerform] = useState<MacPerformReport | null>(null);
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [busy, setBusy] = useState<"plan" | "perform" | "adopt" | "install" | null>(null);
  const [error, setError] = useState<string | null>(null);
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
  const scopeRef = useRef<HTMLDivElement>(null);
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

  const check = useCallback(async () => {
    setBusy("plan");
    setError(null);
    try {
      setReport(await managerApi.macPlanUpdate());
    } catch (cause) {
      // Drop any stale plan so a failed re-check can't keep driving "立即更新"
      // off an outdated currentBuild/latestBuild.
      setReport(null);
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  }, []);

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

  const adopt = useCallback(async () => {
    setBusy("adopt");
    setError(null);
    try {
      setStatus(await managerApi.macAdopt());
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  }, []);

  const runInstall = useCallback(async () => {
    setBusy("install");
    setError(null);
    const un = await startDlListen();
    try {
      setStatus(await managerApi.macInstall());
      setJustInstalled(true);
      await check();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      un();
      setBusy(null);
      setDl(null);
    }
  }, [check, startDlListen]);

  const runPerform = useCallback(async () => {
    const installed = status?.installed;
    const plan = report?.plan;
    if (!installed || !plan || plan.upToDate) return;
    setBusy("perform");
    setError(null);
    const un = await startDlListen();
    try {
      const result = await managerApi.macPerformUpdate({
        fromBuild: installed.build,
        toBuild: plan.latestBuild,
        path: installed.path,
      });
      setPerform(result);
      setConfirmOpen(false);
      await refreshStatus();
      await check();
    } catch (cause) {
      setError(errorMessage(cause));
      setConfirmOpen(false);
    } finally {
      un();
      setBusy(null);
      setDl(null);
    }
  }, [status, report, refreshStatus, check, startDlListen]);

  const plan = report?.plan ?? null;
  const installed = status?.installed ?? report?.installed ?? null;
  const isManaged = status?.status === "managed";
  const updateAvailable = Boolean(plan) && !plan?.upToDate;

  const kind: Kind = useMemo(() => {
    if (!installed) {
      if (busy === "plan" || !statusLoaded) return "loading";
      // A failed status (unsupported platform) OR a failed check (OS too old /
      // appcast unreachable) → error, never an install entry that would just
      // fail again.
      if (statusFailed || error) return "error";
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
    if (error && !report) return "error";
    if (!report) return "idle";
    if (updateAvailable) return "update";
    return "uptodate";
  }, [busy, report, error, installed, updateAvailable, status, statusLoaded, statusFailed]);

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
    void managerApi.macLaunch().catch((cause) => setError(errorMessage(cause)));
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
  // Char-split only LTR scripts — splitting a cursive RTL script (Arabic) into
  // per-char elements breaks its contextual letter joining.
  const splitHeadline = !isShimmer && dirOf(lang) === "ltr";
  useHomeMotion(scopeRef, scene, { splitHeadline, success });

  // ── progress (performing / installing) takes over the whole screen ─────────
  if (busy === "perform" || busy === "install") {
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

  // Fresh-install completion — opening Codex is the user's explicit choice.
  if (justInstalled) {
    return (
      <div className="pop">
        <TopBar />
        <div className="scroll" ref={scopeRef}>
          {error ? (
            <div className="banner err">
              <Icon name="alert" />
              <span>{error}</span>
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

      <div className="scroll" ref={scopeRef}>
        {perform ? (
          <div className={`banner ${perform.rolledBack ? "err" : "ok"}`}>
            <Icon name={perform.rolledBack ? "alert" : "check"} />
            {/* Backend message carries the full outcome: relaunch-failed +
                backup kept, provenance save failure, rollback, etc. */}
            <span>{perform.message}</span>
          </div>
        ) : null}
        {error ? (
          <div className="banner err">
            <Icon name="alert" />
            <span>{error}</span>
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
            <Ring icon="arrowUp" className="" />
            <h3>{t("confirm.title", { version: latestVersion })}</h3>
            <p>{t("confirm.body")}</p>
            <div className="row2">
              <button className="btn ghost" onClick={() => setConfirmOpen(false)}>
                {t("confirm.cancel")}
              </button>
              <button className="btn primary" onClick={runPerform}>
                {t("confirm.ok")}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
