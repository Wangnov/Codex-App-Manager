import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { errorMessage, managerApi, SETTINGS_CHANGED_EVENT } from "../../services/managerApi";
import type {
  AppSettings,
  CatalogSkin,
  CodexThemeStatusReport,
  CodexThemeSummary,
} from "../../shared/types";
import { NavBar, Segmented, StatusBanner } from "../components";
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

/** Screenshot cover, lazily delivered as a data URL through the backend.
 *  Clicking a loaded cover opens the lightbox (`onZoom`). */
function PhotoCover({
  load,
  onZoom,
  zoomLabel,
}: {
  load: () => Promise<string | null>;
  onZoom: (dataUrl: string) => void;
  zoomLabel: string;
}) {
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const loadRef = useRef(load);
  loadRef.current = load;
  useEffect(() => {
    let cancelled = false;
    void loadRef
      .current()
      .then((url) => {
        if (!cancelled) setDataUrl(url);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);
  return (
    <button
      type="button"
      className="themecard-art themecard-art-photo"
      title={zoomLabel}
      disabled={!dataUrl}
      onClick={() => {
        if (dataUrl) onZoom(dataUrl);
      }}
    >
      {dataUrl ? <img src={dataUrl} alt="" draggable={false} /> : null}
    </button>
  );
}

/** Full-window preview overlay. Click anywhere or press Esc to dismiss. */
function Lightbox({ src, onClose }: { src: string | null; onClose: () => void }) {
  useEffect(() => {
    if (!src) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [src, onClose]);
  if (!src) return null;
  return (
    // Backdrop click-to-dismiss is a mouse convenience; the keyboard path is
    // Esc above. The close button carries the accessible control.
    // eslint-disable-next-line jsx-a11y/no-static-element-interactions, jsx-a11y/click-events-have-key-events
    <div className="lightbox" onClick={onClose}>
      <img src={src} alt="" draggable={false} />
      <button className="lightbox-close" onClick={onClose} aria-label="close">
        <Icon name="close" />
      </button>
    </div>
  );
}

/** Case-insensitive multi-field match: name, id, description, author, tags,
 *  appearance and version all participate — not just the title. */
function matches(query: string, fields: Array<string | null | undefined>): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return q
    .split(/\s+/)
    .every((token) => fields.some((f) => (f ?? "").toLowerCase().includes(token)));
}

type GalleryTab = "local" | "store";

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
  const [tab, setTab] = useState<GalleryTab>("local");
  const [query, setQuery] = useState("");
  const [lightbox, setLightbox] = useState<string | null>(null);

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
  // The kept theme has two live states: injected (daemon confirms it) and
  // paused (off-live keeps the selection and the native palette; only the
  // CSS layer is withdrawn for this session).
  const activeInjected = Boolean(activeId && status?.daemon?.themeId === activeId);
  const activePaused = Boolean(activeId && !status?.daemon?.themeId);
  const themeName = (id: string | null) =>
    (id && themes.find((theme) => theme.id === id)?.name) || id || "";

  // The backend lists every root (dev first, then store) WITHOUT deduping so
  // the store tab can see the store copy behind a same-id dev checkout. The
  // local gallery shows resolution order: first occurrence per id wins.
  const localThemes = useMemo(() => {
    const seen = new Set<string>();
    return themes.filter((theme) => (seen.has(theme.id) ? false : (seen.add(theme.id), true)));
  }, [themes]);

  const visibleThemes = localThemes.filter((theme) =>
    matches(query, [
      theme.name,
      theme.id,
      theme.description,
      theme.meta.author,
      theme.meta.version,
      theme.meta.appearance,
      theme.meta.codexVerified,
      ...theme.meta.tags,
    ]),
  );
  const visibleCatalog = (catalog ?? []).filter((skin) =>
    matches(query, [
      skin.name,
      skin.id,
      skin.description,
      skin.author,
      skin.version,
      skin.appearance,
      skin.codexVerified,
      ...skin.tags,
    ]),
  );

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
      <NavBar
        title={t("themes.title")}
        onBack={onBack}
        disableBack={busy !== null && busy.startsWith("tryon-restart")}
      >
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
              <span className="row-actions">
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
                  onClick={() => void run("cancel", () => managerApi.codexThemeCancel())}
                >
                  {t("themes.revert")}
                </button>
              </span>
            }
          >
            {t("themes.status.tryingOn", { name: themeName(tryingId) })}
          </StatusBanner>
        ) : null}

        {status?.recoveryRequired ? (
          <StatusBanner tone="err">{t("themes.status.recovery")}</StatusBanner>
        ) : null}

        {activeId && activeInjected ? (
          <StatusBanner
            tone="ok"
            action={
              <span className="row-actions">
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

        {activeId && activePaused ? (
          <StatusBanner
            tone="info"
            action={
              <span className="row-actions">
                <button
                  className="btn primary sm"
                  disabled={busy !== null || !status?.supported}
                  onClick={() =>
                    void run(`apply:${activeId}`, () => managerApi.codexThemeApply(activeId))
                  }
                >
                  {busy === `apply:${activeId}` ? t("themes.busy.tryOn") : t("themes.enable")}
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
            {t("themes.status.paused", { name: themeName(activeId) })}
          </StatusBanner>
        ) : null}

        <div className="themes-toolbar">
          <Segmented
            ariaLabel={t("themes.title")}
            value={tab}
            items={[
              { key: "local", label: t("themes.tab.local") },
              { key: "store", label: t("themes.tab.store") },
            ]}
            onChange={(key) => setTab(key as GalleryTab)}
          />
          <div className="themes-search">
            <Icon name="search" />
            <input
              type="search"
              value={query}
              placeholder={t("themes.search")}
              aria-label={t("themes.search")}
              onChange={(event) => setQuery(event.target.value)}
            />
          </div>
        </div>

        {tab === "local" ? (
          <>
            <div className="themegrid">
              {visibleThemes.map((theme) => {
                const isActive = theme.id === activeId;
                const isTrying = theme.id === tryingId;
                return (
                  <article key={theme.id} className={`themecard${isActive ? " active" : ""}`}>
                    {theme.preview ? (
                      <PhotoCover
                        load={() => managerApi.codexThemePreview(theme.id)}
                        onZoom={setLightbox}
                        zoomLabel={t("themes.zoom")}
                      />
                    ) : (
                      <ThemeCardArt colors={theme.colors} />
                    )}
                    <div className="themecard-body">
                      <div className="themecard-head">
                        <span className="themecard-name">{theme.name}</span>
                        {theme.meta.version ? (
                          <span className="themecard-version">v{theme.meta.version}</span>
                        ) : null}
                        {theme.origin === "dev" ? (
                          <span className="tag soon" title={t("themes.origin.devHint")}>
                            {t("themes.origin.dev")}
                          </span>
                        ) : null}
                        {isActive ? <span className="tag ok">{t("themes.inUse")}</span> : null}
                        {isTrying ? <span className="tag soon">{t("themes.trying")}</span> : null}
                        {theme.meta.codexVerified ? (
                          <span
                            className="tag soon"
                            title={t("themes.verifiedHint", { v: theme.meta.codexVerified })}
                          >
                            @{theme.meta.codexVerified.split(".").slice(0, 2).join(".")}
                          </span>
                        ) : null}
                      </div>
                      {theme.meta.author ? (
                        <span className="themecard-author">@{theme.meta.author}</span>
                      ) : null}
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
                      {!isActive && !isTrying ? (
                        <div className="themecard-actions">
                          <button
                            className="btn primary sm"
                            disabled={busy !== null || !status?.supported}
                            onClick={() =>
                              void run(
                                status?.cdpReady
                                  ? `tryon:${theme.id}`
                                  : `tryon-restart:${theme.id}`,
                                () =>
                                  status?.cdpReady
                                    ? managerApi.codexThemeTryOn(theme.id)
                                    : managerApi.codexThemeTryOnRestart(theme.id),
                              )
                            }
                          >
                            {busy === `tryon:${theme.id}` || busy === `tryon-restart:${theme.id}`
                              ? t("themes.busy.tryOn")
                              : status?.cdpReady
                                ? t("themes.tryOn")
                                : t("themes.tryOnRestart")}
                          </button>
                        </div>
                      ) : null}
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
            {loaded && themes.length > 0 && visibleThemes.length === 0 ? (
              <p className="themes-noresult">{t("themes.noResults")}</p>
            ) : null}

            <div className="group-h">{t("themes.storage.header")}</div>
            <div className="list">
              <div className="row">
                <Icon name="folder" className="ricon" />
                <span className="rtext">
                  <span className="rtitle">{t("themes.storage.title")}</span>
                  <span className="rsub mono-path" title={status?.storeDir ?? undefined}>
                    {status?.storeDir ?? "…"}
                  </span>
                </span>
                <span className="row-actions">
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
                <div className="row">
                  <span className="rsub" role="status">{storeNote}</span>
                </div>
              ) : null}
            </div>

            <div className="group-h">{t("themes.devdir.title")}</div>
            <div className="list">
              <div className="row">
                <Icon name="sliders" className="ricon" />
                <span className="rtext">
                  <span className="rtitle">{t("themes.devdir.title")}</span>
                  <span className="rsub">{t("themes.devdir.sub")}</span>
                </span>
              </div>
              <div className="row devdir-edit">
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
          </>
        ) : (
          <>
            {catalogFailed ? (
              <StatusBanner tone="info">{t("themes.online.offline")}</StatusBanner>
            ) : catalog === null ? (
              <p className="themes-noresult">{t("themes.online.loading")}</p>
            ) : (
              <>
                <div className="themegrid">
                  {visibleCatalog.map((skin) => {
                    // Compare against the STORE copy specifically — a same-id
                    // dev checkout shadows it in resolution order but says
                    // nothing about what the store has installed.
                    const installed = themes.find(
                      (theme) => theme.id === skin.id && theme.origin === "store",
                    );
                    const upToDate = installed && installed.meta.version === skin.version;
                    const isUpgrade = installed && installed.meta.version !== skin.version;
                    const busyKey = `online:${skin.id}`;
                    return (
                      <article key={skin.id} className="themecard">
                        <PhotoCover
                          load={() => managerApi.codexThemeCatalogPreview(skin.preview)}
                          onZoom={setLightbox}
                          zoomLabel={t("themes.zoom")}
                        />
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
                          {skin.author ? (
                            <span className="themecard-author">@{skin.author}</span>
                          ) : null}
                          {skin.description ? (
                            <p className="themecard-desc">{skin.description}</p>
                          ) : null}
                          {!upToDate ? (
                            <div className="themecard-actions">
                              <button
                                className="btn primary sm"
                                disabled={busy !== null}
                                onClick={() =>
                                  void run(busyKey, () =>
                                    managerApi.codexThemeInstallOnline(skin.id),
                                  )
                                }
                              >
                                {busy === busyKey
                                  ? t("themes.online.installing")
                                  : isUpgrade
                                    ? t("themes.online.update", { v: skin.version })
                                    : t("themes.online.install")}
                              </button>
                            </div>
                          ) : null}
                        </div>
                      </article>
                    );
                  })}
                </div>
                {catalog.length > 0 && visibleCatalog.length === 0 ? (
                  <p className="themes-noresult">{t("themes.noResults")}</p>
                ) : null}
              </>
            )}
          </>
        )}
      </div>
      <Lightbox src={lightbox} onClose={() => setLightbox(null)} />
    </div>
  );
}
