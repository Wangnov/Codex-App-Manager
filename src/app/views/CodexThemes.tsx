import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { errorMessage, managerApi, SETTINGS_CHANGED_EVENT } from "../../services/managerApi";
import type {
  AppSettings,
  CatalogSkin,
  CodexThemeStatusReport,
  CodexThemeSummary,
} from "../../shared/types";
import { NavBar, StatusBanner } from "../components";
import { Icon } from "../icons";
import { useI18n } from "../i18n";

function isTauri(): boolean {
  return (
    typeof window !== "undefined" &&
    Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
  );
}

/** Parse #rgb/#rrggbb/rgb() far enough for luminance/saturation ranking. */
function parseColor(value: string): { r: number; g: number; b: number } | null {
  const hex = value.trim().match(/^#([0-9a-f]{3,8})$/i)?.[1];
  if (hex) {
    const size = hex.length >= 6 ? 2 : 1;
    const chan = (i: number) => {
      const raw = hex.slice(i * size, i * size + size);
      const parsed = parseInt(size === 1 ? raw + raw : raw, 16);
      return Number.isNaN(parsed) ? null : parsed;
    };
    const [r, g, b] = [chan(0), chan(1), chan(2)];
    return r == null || g == null || b == null ? null : { r, g, b };
  }
  const rgb = value.trim().match(/^rgba?\(([^)]+)\)$/i)?.[1];
  if (rgb) {
    const [r, g, b] = rgb.split(",").map((part) => Number(part.trim().replace("%", "")));
    if ([r, g, b].every((n) => Number.isFinite(n))) return { r, g, b };
  }
  return null;
}

/** Theme packages name their colors freely (cream/amber/lcl/...), so the card
 *  art derives roles from the values instead: darkest = backdrop, most
 *  saturated = accent, lightest = ink. Every theme card becomes its own
 *  poster with zero bundled artwork. */
function cardPalette(colors: Record<string, string>) {
  const parsed = Object.values(colors)
    .map((value) => ({ value, rgb: parseColor(value) }))
    .filter((c): c is { value: string; rgb: { r: number; g: number; b: number } } => c.rgb !== null)
    .map(({ value, rgb }) => {
      const max = Math.max(rgb.r, rgb.g, rgb.b);
      const min = Math.min(rgb.r, rgb.g, rgb.b);
      return {
        value,
        luminance: (0.2126 * rgb.r + 0.7152 * rgb.g + 0.0722 * rgb.b) / 255,
        saturation: max === 0 ? 0 : (max - min) / max,
      };
    });
  if (!parsed.length) {
    return { backdrop: "var(--surface-2)", panel: "var(--surface-3)", accent: "var(--accent)", ink: "var(--text)" };
  }
  const byLuminance = [...parsed].sort((a, b) => a.luminance - b.luminance);
  const accent = [...parsed].sort(
    (a, b) => b.saturation * (1 - Math.abs(b.luminance - 0.55)) - a.saturation * (1 - Math.abs(a.luminance - 0.55)),
  )[0];
  return {
    backdrop: byLuminance[0].value,
    panel: byLuminance[Math.min(1, byLuminance.length - 1)].value,
    accent: accent.value,
    ink: byLuminance[byLuminance.length - 1].value,
  };
}

function ThemeCardArt({ colors }: { colors: Record<string, string> }) {
  const palette = useMemo(() => cardPalette(colors), [colors]);
  return (
    <div className="themecard-art" style={{ background: palette.backdrop }} aria-hidden="true">
      <span className="tca-side" style={{ background: palette.panel }} />
      <span className="tca-line" style={{ background: palette.ink, opacity: 0.55 }} />
      <span className="tca-line tca-line-2" style={{ background: palette.ink, opacity: 0.3 }} />
      <span className="tca-composer" style={{ borderColor: palette.accent }}>
        <span className="tca-send" style={{ background: palette.accent }} />
      </span>
    </div>
  );
}

/** Online catalog cover: fetched through the backend (pinned origin, curl)
 *  as a data URL, so the CSP needs no remote img-src. */
function CatalogCardCover({ preview }: { preview: string }) {
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    void managerApi
      .codexThemeCatalogPreview(preview)
      .then((url) => {
        if (!cancelled) setDataUrl(url);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [preview]);
  return (
    <div className="themecard-art themecard-art-photo">
      {dataUrl ? <img src={dataUrl} alt="" draggable={false} /> : null}
    </div>
  );
}

/** Real screenshot when the package ships one (delivered lazily as a data
 *  URL), abstract swatch art otherwise — so undelivered/in-development
 *  themes still have a face. */
function ThemeCardCover({ theme }: { theme: CodexThemeSummary }) {
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const hasPreview = Boolean(theme.preview);
  useEffect(() => {
    if (!hasPreview) return;
    let cancelled = false;
    void managerApi
      .codexThemePreview(theme.id)
      .then((url) => {
        if (!cancelled) setDataUrl(url);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [hasPreview, theme.id]);
  if (dataUrl) {
    return (
      <div className="themecard-art themecard-art-photo">
        <img src={dataUrl} alt="" draggable={false} />
      </div>
    );
  }
  return <ThemeCardArt colors={theme.colors} />;
}

export function CodexThemes({ onBack }: { onBack: () => void }) {
  const { t } = useI18n();
  const [themes, setThemes] = useState<CodexThemeSummary[]>([]);
  const [status, setStatus] = useState<CodexThemeStatusReport | null>(null);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [devDirDraft, setDevDirDraft] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [catalog, setCatalog] = useState<CatalogSkin[] | null>(null);
  const [catalogFailed, setCatalogFailed] = useState(false);
  const [storeNote, setStoreNote] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [list, report, current] = await Promise.all([
        managerApi.codexThemeList(),
        managerApi.codexThemeStatus(),
        managerApi.getSettings(),
      ]);
      setThemes(list);
      setStatus(report);
      setSettings(current);
      setDevDirDraft(current.codexThemeDir ?? "");
    } catch (cause) {
      setActionError(errorMessage(cause));
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const onSettings = () => void refresh();
    window.addEventListener(SETTINGS_CHANGED_EVENT, onSettings);
    return () => window.removeEventListener(SETTINGS_CHANGED_EVENT, onSettings);
  }, [refresh]);

  // The online catalog loads independently of the local gallery — network
  // latency (or being offline) must never hold the local list hostage.
  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    void managerApi
      .codexThemeCatalog()
      .then((skins) => {
        if (!cancelled) setCatalog(skins);
      })
      .catch(() => {
        if (!cancelled) setCatalogFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Keep the status line live while the view is open (daemon connects targets
  // asynchronously after an apply).
  useEffect(() => {
    const id = window.setInterval(() => {
      void managerApi.codexThemeStatus().then(setStatus).catch(() => undefined);
    }, 3000);
    return () => window.clearInterval(id);
  }, []);

  const run = useCallback(
    async (key: string, action: () => Promise<unknown>) => {
      if (busy) return;
      setBusy(key);
      setActionError(null);
      try {
        await action();
        await refresh();
      } catch (cause) {
        setActionError(errorMessage(cause));
      } finally {
        setBusy(null);
      }
    },
    [busy, refresh],
  );

  const activeId = status?.activeTheme ?? null;
  const tryingId = status?.daemon?.themeId && status.daemon.themeId !== activeId
    ? status.daemon.themeId
    : null;
  const themeName = (id: string | null) =>
    (id && themes.find((theme) => theme.id === id)?.name) || id || "";

  const saveDevDir = () =>
    run("devdir", async () => {
      const current = settings ?? (await managerApi.getSettings());
      await managerApi.setSettings({
        ...current,
        codexThemeDir: devDirDraft.trim() ? devDirDraft.trim() : null,
      });
    });

  const importSkin = () =>
    run("import", async () => {
      await managerApi.codexThemeImport();
    });

  // Drag-and-drop delivery: Tauri intercepts native file drops and rebroadcasts
  // them as webview events; install every dropped .codexskin.
  const dropHandler = useRef<(paths: string[]) => void>(() => undefined);
  dropHandler.current = (paths) => {
    const skins = paths.filter((p) => p.toLowerCase().endsWith(".codexskin"));
    if (!skins.length) return;
    void run("import", async () => {
      for (const skin of skins) {
        await managerApi.codexThemeImportPath(skin);
      }
    });
  };
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void import("@tauri-apps/api/webview")
      .then(({ getCurrentWebview }) =>
        getCurrentWebview().onDragDropEvent((event) => {
          if (event.payload.type === "drop") {
            dropHandler.current(event.payload.paths);
          }
        }),
      )
      .then((dispose) => {
        if (disposed) dispose();
        else unlisten = dispose;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return (
    <div className="pop">
      <NavBar title={t("themes.title")} onBack={onBack} disableBack={busy === "apply"}>
        <button
          className="iconbtn"
          title={t("themes.import")}
          disabled={busy !== null}
          onClick={() => void importSkin()}
        >
          <Icon name="download" />
        </button>
      </NavBar>
      <div className="scroll scroll-wide view">
        {status && !status.supported ? (
          <StatusBanner tone="info">{t("themes.status.unsupported")}</StatusBanner>
        ) : null}
        {actionError ? <StatusBanner tone="err">{actionError}</StatusBanner> : null}
        {status?.daemon?.lastError && !actionError ? (
          <StatusBanner tone="warn">
            {t("themes.status.daemonError", { error: status.daemon.lastError })}
          </StatusBanner>
        ) : null}

        {tryingId ? (
          <StatusBanner
            tone="info"
            icon="sliders"
            action={
              <span className="row2" style={{ gap: 8 }}>
                <button
                  className="btn primary sm"
                  disabled={busy !== null}
                  onClick={() => void run("keep", () => managerApi.codexThemeKeep(tryingId))}
                >
                  {t("themes.keep")}
                </button>
                <button
                  className="btn ghost sm"
                  disabled={busy !== null}
                  onClick={() => void run("offlive", () => managerApi.codexThemeOff(false))}
                >
                  {t("themes.revert")}
                </button>
              </span>
            }
          >
            {t("themes.status.tryingOn", { name: themeName(tryingId) })}
          </StatusBanner>
        ) : null}

        {activeId ? (
          <StatusBanner
            tone="ok"
            action={
              <span className="row2" style={{ gap: 8 }}>
                <button
                  className="btn ghost sm"
                  disabled={busy !== null}
                  onClick={() => void run("offlive", () => managerApi.codexThemeOff(false))}
                >
                  {t("themes.turnOff")}
                </button>
                {status?.nativeBackupPresent ? (
                  <button
                    className="btn ghost sm"
                    disabled={busy !== null}
                    onClick={() => void run("offfull", () => managerApi.codexThemeOff(true))}
                  >
                    {t("themes.restoreFull")}
                  </button>
                ) : null}
              </span>
            }
          >
            {t("themes.status.active", { name: themeName(activeId) })}
          </StatusBanner>
        ) : null}

        {status?.supported && themes.length > 0 && !status.cdpReady ? (
          <StatusBanner tone="info">{t("themes.status.needsDebug")}</StatusBanner>
        ) : null}

        <div className="themegrid">
          {themes.map((theme) => {
            const isActive = theme.id === activeId;
            const isTrying = theme.id === tryingId;
            return (
              <article key={theme.id} className={`themecard${isActive ? " active" : ""}`}>
                <ThemeCardCover theme={theme} />
                <div className="themecard-body">
                  <div className="themecard-head">
                    <span className="themecard-name">{theme.name}</span>
                    {theme.meta.version ? (
                      <span className="themecard-version">v{theme.meta.version}</span>
                    ) : null}
                    {isActive ? <span className="tag ok">{t("themes.inUse")}</span> : null}
                    {isTrying ? <span className="tag soon">{t("themes.trying")}</span> : null}
                    {theme.hasNativeTheme ? (
                      <span className="tag" title={t("themes.nativeHint")}>
                        {t("themes.native")}
                      </span>
                    ) : null}
                    {theme.meta.codexVerified ? (
                      <span
                        className="tag soon"
                        title={t("themes.verifiedHint", { v: theme.meta.codexVerified })}
                      >
                        @{theme.meta.codexVerified.split(".").slice(0, 2).join(".")}
                      </span>
                    ) : null}
                  </div>
                  {theme.description ? (
                    <p className="themecard-desc">{theme.description}</p>
                  ) : null}
                  <div className="themecard-swatches" aria-hidden="true">
                    {Object.entries(theme.colors)
                      .slice(0, 10)
                      .map(([key, value]) => (
                        <span key={key} className="swatch" style={{ background: value }} title={key} />
                      ))}
                  </div>
                  <div className="themecard-actions">
                    {status?.cdpReady && !isActive ? (
                      <button
                        className="btn ghost sm"
                        disabled={busy !== null}
                        onClick={() =>
                          void run("tryon", () => managerApi.codexThemeTryOn(theme.id))
                        }
                      >
                        {busy === "tryon" ? t("themes.busy.tryOn") : t("themes.tryOn")}
                      </button>
                    ) : null}
                    {!isActive ? (
                      <button
                        className="btn primary sm"
                        disabled={busy !== null || !status?.supported}
                        onClick={() =>
                          void run("apply", () => managerApi.codexThemeApply(theme.id))
                        }
                      >
                        {busy === "apply" ? t("themes.busy.apply") : t("themes.applyRestart")}
                      </button>
                    ) : (
                      <button
                        className="btn ghost sm"
                        disabled={busy !== null}
                        onClick={() =>
                          void run("apply", () => managerApi.codexThemeApply(theme.id))
                        }
                      >
                        {t("themes.reapply")}
                      </button>
                    )}
                  </div>
                </div>
              </article>
            );
          })}
        </div>

        {loaded && themes.length === 0 ? (
          <section className="hero" style={{ paddingTop: 24 }}>
            <Icon name="sliders" className="ricon" />
            <div className="headline" style={{ fontSize: 16 }}>
              {t("themes.empty.title")}
            </div>
            <div className="desc">{t("themes.empty.sub")}</div>
          </section>
        ) : null}

        {catalog !== null || catalogFailed ? (
          <>
            <div className="group-h">{t("themes.online.header")}</div>
            {catalogFailed ? (
              <StatusBanner tone="info">{t("themes.online.offline")}</StatusBanner>
            ) : (
              <div className="themegrid">
                {(catalog ?? []).map((skin) => {
                  const installed = themes.find((theme) => theme.id === skin.id);
                  const upToDate = installed && installed.meta.version === skin.version;
                  const isUpgrade = installed && installed.meta.version !== skin.version;
                  return (
                    <article key={skin.id} className="themecard">
                      <CatalogCardCover preview={skin.preview} />
                      <div className="themecard-body">
                        <div className="themecard-head">
                          <span className="themecard-name">{skin.name}</span>
                          <span className="themecard-version">v{skin.version}</span>
                          {skin.codexVerified ? (
                            <span
                              className="tag soon"
                              title={t("themes.verifiedHint", { v: skin.codexVerified })}
                            >
                              @{skin.codexVerified.split(".").slice(0, 2).join(".")}
                            </span>
                          ) : null}
                          {upToDate ? (
                            <span className="tag ok">{t("themes.online.installed")}</span>
                          ) : null}
                        </div>
                        {skin.description ? (
                          <p className="themecard-desc">{skin.description}</p>
                        ) : null}
                        <div className="themecard-actions">
                          {!upToDate ? (
                            <button
                              className="btn primary sm"
                              disabled={busy !== null}
                              onClick={() =>
                                void run("online", () =>
                                  managerApi.codexThemeInstallOnline(skin.id),
                                )
                              }
                            >
                              {busy === "online"
                                ? t("themes.online.installing")
                                : isUpgrade
                                  ? t("themes.online.update", { v: skin.version })
                                  : t("themes.online.install")}
                            </button>
                          ) : null}
                        </div>
                      </div>
                    </article>
                  );
                })}
              </div>
            )}
          </>
        ) : null}

        <div className="group-h">{t("themes.storage.header")}</div>
        <div className="list">
          <div className="row">
            <Icon name="folder" className="ricon" />
            <span className="rtext">
              <span className="rtitle">{t("themes.storage.title")}</span>
              <span className="rsub mono-path">{status?.storeDir ?? "…"}</span>
            </span>
            <span className="row2" style={{ gap: 8 }}>
              <button
                className="btn ghost sm"
                disabled={busy !== null}
                onClick={() => void run("store", async () => {
                  const report = await managerApi.codexThemePickStoreDir();
                  if (report) {
                    setActionError(null);
                    setStoreNote(
                      t("themes.storage.migrated", {
                        n: String(report.moved.length),
                        skipped: report.skipped.length
                          ? t("themes.storage.skipped", { m: String(report.skipped.length) })
                          : "",
                      }),
                    );
                  }
                })}
              >
                {t("themes.storage.change")}
              </button>
              <button
                className="btn ghost sm"
                onClick={() => void managerApi.codexThemeOpenStore()}
              >
                {t("themes.storage.open")}
              </button>
            </span>
          </div>
          {storeNote ? (
            <div className="row" style={{ display: "block" }}>
              <span className="rsub" role="status">{storeNote}</span>
            </div>
          ) : null}
        </div>

        <div className="group-h">{t("themes.devdir.title")}</div>
        <div className="list">
          <div className="row" style={{ display: "block" }}>
            <span
              className="rtext"
              style={{ display: "flex", flexDirection: "column", marginBottom: 8 }}
            >
              <span className="rtitle">{t("themes.devdir.title")}</span>
              <span className="rsub">{t("themes.devdir.sub")}</span>
            </span>
            <div className="row2" style={{ gap: 8 }}>
              <input
                className="input mono"
                aria-label={t("themes.devdir.title")}
                value={devDirDraft}
                placeholder={t("themes.devdir.placeholder")}
                onChange={(event) => setDevDirDraft(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") void saveDevDir();
                }}
              />
              <button
                className="btn ghost sm"
                disabled={busy !== null || (settings?.codexThemeDir ?? "") === devDirDraft.trim()}
                onClick={() => void saveDevDir()}
              >
                {t("themes.devdir.save")}
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
