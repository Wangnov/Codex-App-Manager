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
import { Sheet } from "../Sheet";
import { useI18n, type TFn } from "../i18n";

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

/** Theme packages name their colors freely, so the card art derives roles from
 *  the values: darkest = backdrop, most saturated = accent, lightest = ink. */
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
  className = "themecard-art themecard-art-photo",
}: {
  load: () => Promise<string | null>;
  onZoom: (dataUrl: string) => void;
  zoomLabel: string;
  className?: string;
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
      className={className}
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
    // eslint-disable-next-line jsx-a11y/no-static-element-interactions, jsx-a11y/click-events-have-key-events
    <div className="lightbox" onClick={onClose}>
      <img src={src} alt="" draggable={false} />
      <button className="lightbox-close" onClick={onClose} aria-label="close">
        <Icon name="close" />
      </button>
    </div>
  );
}

/** Case-insensitive multi-field match. */
function matches(query: string, fields: Array<string | null | undefined>): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return q.split(/\s+/).every((token) => fields.some((f) => (f ?? "").toLowerCase().includes(token)));
}

type GalleryTab = "local" | "store";
type ViewMode = "card" | "list";
const VIEW_KEY = "cam.skins.view";
const PAGE_SIZE: Record<ViewMode, number> = { card: 12, list: 20 };

/** One row in the gallery, normalized across a local package and a catalog
 *  entry so the card/list/detail chrome is shared. */
interface Item {
  kind: GalleryTab;
  id: string;
  name: string;
  version: string | null;
  author: string | null;
  description: string;
  codexVerified: string | null;
  appearance: string | null;
  license: string | null;
  tags: string[];
  colors: Record<string, string>;
  hasPreview: boolean;
  loadPreview: () => Promise<string | null>;
  origin?: "dev" | "store"; // local only
  installedVersion?: string | null; // store only: version present in the store
}

function localItem(theme: CodexThemeSummary): Item {
  return {
    kind: "local",
    id: theme.id,
    name: theme.name,
    version: theme.meta.version,
    author: theme.meta.author,
    description: theme.description,
    codexVerified: theme.meta.codexVerified,
    appearance: theme.meta.appearance,
    license: theme.meta.license,
    tags: theme.meta.tags,
    colors: theme.colors,
    hasPreview: Boolean(theme.preview),
    loadPreview: () => managerApi.codexThemePreview(theme.id),
    origin: theme.origin,
  };
}

function storeItem(skin: CatalogSkin, installedVersion: string | null): Item {
  return {
    kind: "store",
    id: skin.id,
    name: skin.name,
    version: skin.version,
    author: skin.author,
    description: skin.description,
    codexVerified: skin.codexVerified,
    appearance: skin.appearance,
    license: skin.license,
    tags: skin.tags,
    colors: {},
    hasPreview: true,
    loadPreview: () => managerApi.codexThemeCatalogPreview(skin.preview),
    installedVersion,
  };
}

/** Numbered pager. Renders nothing when everything fits on one page. */
function Pagination({
  page,
  pages,
  onPage,
  label,
}: {
  page: number;
  pages: number;
  onPage: (p: number) => void;
  label: TFn;
}) {
  if (pages <= 1) return null;
  const nums = Array.from({ length: pages }, (_, i) => i).filter(
    (i) => i === 0 || i === pages - 1 || Math.abs(i - page) <= 1,
  );
  const out: Array<number | "gap"> = [];
  nums.forEach((n, i) => {
    if (i > 0 && n - nums[i - 1] > 1) out.push("gap");
    out.push(n);
  });
  return (
    <nav className="pager" aria-label={label("themes.page.nav")}>
      <button
        className="pager-btn"
        disabled={page === 0}
        onClick={() => onPage(page - 1)}
        aria-label={label("themes.page.prev")}
      >
        <Icon name="back" />
      </button>
      {out.map((n, i) =>
        n === "gap" ? (
          <span key={`gap${i}`} className="pager-gap">
            …
          </span>
        ) : (
          <button
            key={n}
            className={`pager-btn${n === page ? " active" : ""}`}
            aria-current={n === page ? "page" : undefined}
            onClick={() => onPage(n)}
          >
            {n + 1}
          </button>
        ),
      )}
      <button
        className="pager-btn"
        disabled={page >= pages - 1}
        onClick={() => onPage(page + 1)}
        aria-label={label("themes.page.next")}
      >
        <Icon name="chevron" />
      </button>
    </nav>
  );
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
  const [tab, setTab] = useState<GalleryTab>("local");
  const [query, setQuery] = useState("");
  const [lightbox, setLightbox] = useState<string | null>(null);
  const [view, setView] = useState<ViewMode>(() => {
    try {
      return localStorage.getItem(VIEW_KEY) === "list" ? "list" : "card";
    } catch {
      return "card";
    }
  });
  const [page, setPage] = useState(0);
  const [selecting, setSelecting] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [detail, setDetail] = useState<Item | null>(null);
  const [confirmIds, setConfirmIds] = useState<string[] | null>(null);

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

  useEffect(() => {
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

  useEffect(() => {
    const id = window.setInterval(() => {
      void managerApi.codexThemeStatus().then(setStatus).catch(() => undefined);
    }, 3000);
    return () => window.clearInterval(id);
  }, []);

  const setViewMode = useCallback((next: ViewMode) => {
    setView(next);
    setPage(0);
    try {
      localStorage.setItem(VIEW_KEY, next);
    } catch {
      // view memory is a nicety
    }
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
  const tryingId =
    status?.daemon?.themeId && status.daemon.themeId !== activeId ? status.daemon.themeId : null;
  const activeInjected = Boolean(activeId && status?.daemon?.themeId === activeId);
  const activePaused = Boolean(activeId && !status?.daemon?.themeId);

  // Dedup local list to resolution order (dev shadows store) for display.
  const localThemes = useMemo(() => {
    const seen = new Set<string>();
    return themes.filter((theme) => (seen.has(theme.id) ? false : (seen.add(theme.id), true)));
  }, [themes]);

  const storeVersionOf = useCallback(
    (id: string) => themes.find((th) => th.id === id && th.origin === "store")?.meta.version ?? null,
    [themes],
  );

  const items = useMemo<Item[]>(() => {
    if (tab === "local") return localThemes.map(localItem);
    return (catalog ?? []).map((skin) => storeItem(skin, storeVersionOf(skin.id)));
  }, [tab, localThemes, catalog, storeVersionOf]);

  const visible = useMemo(
    () =>
      items.filter((it) =>
        matches(query, [
          it.name,
          it.id,
          it.description,
          it.author,
          it.version,
          it.appearance,
          it.codexVerified,
          ...it.tags,
        ]),
      ),
    [items, query],
  );

  const pageSize = PAGE_SIZE[view];
  const pages = Math.max(1, Math.ceil(visible.length / pageSize));
  // Reset paging when the working set changes underfoot.
  useEffect(() => {
    setPage(0);
  }, [tab, query, view]);
  const clampedPage = Math.min(page, pages - 1);
  const paged = visible.slice(clampedPage * pageSize, clampedPage * pageSize + pageSize);

  const themeName = (id: string | null) =>
    (id && themes.find((theme) => theme.id === id)?.name) || id || "";

  // A store-origin local package that isn't the standing selection can be
  // removed; dev checkouts are the user's source and are protected.
  const deletable = useCallback(
    (it: Item) => it.kind === "local" && it.origin === "store" && it.id !== activeId,
    [activeId],
  );
  const selectableOnPage = paged.filter(deletable);
  const allSelected =
    selectableOnPage.length > 0 && selectableOnPage.every((it) => selected.has(it.id));

  const toggleSelect = (id: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const toggleSelectAll = () =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (allSelected) selectableOnPage.forEach((it) => next.delete(it.id));
      else selectableOnPage.forEach((it) => next.add(it.id));
      return next;
    });
  const exitSelect = () => {
    setSelecting(false);
    setSelected(new Set());
  };

  const doDelete = (ids: string[]) =>
    run("delete", async () => {
      for (const id of ids) await managerApi.codexThemeDelete(id);
      setConfirmIds(null);
      setSelected(new Set());
      setSelecting(false);
      setDetail(null);
    });

  const tryOn = (it: Item) =>
    run(status?.cdpReady ? `tryon:${it.id}` : `tryon-restart:${it.id}`, () =>
      status?.cdpReady
        ? managerApi.codexThemeTryOn(it.id)
        : managerApi.codexThemeTryOnRestart(it.id),
    );
  const installOnline = (it: Item) =>
    run(`online:${it.id}`, () => managerApi.codexThemeInstallOnline(it.id));

  const saveDevDir = () =>
    run("devdir", async () => {
      const current = settings ?? (await managerApi.getSettings());
      await managerApi.setSettings({
        ...current,
        codexThemeDir: devDirDraft.trim() ? devDirDraft.trim() : null,
      });
    });

  const importSkin = () => run("import", async () => void (await managerApi.codexThemeImport()));

  // Drag-and-drop install of dropped .codexskin files.
  const dropHandler = useRef<(paths: string[]) => void>(() => undefined);
  dropHandler.current = (paths) => {
    const skins = paths.filter((p) => p.toLowerCase().endsWith(".codexskin"));
    if (!skins.length) return;
    void run("import", async () => {
      for (const skin of skins) await managerApi.codexThemeImportPath(skin);
    });
  };
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void import("@tauri-apps/api/webview")
      .then(({ getCurrentWebview }) =>
        getCurrentWebview().onDragDropEvent((event) => {
          if (event.payload.type === "drop") dropHandler.current(event.payload.paths);
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

  // ── per-item badges + primary action ──────────────────────────────────────
  const badges = (it: Item) => {
    const out: Array<{ key: string; cls: string; text: string; title?: string }> = [];
    if (it.version) out.push({ key: "v", cls: "version", text: `v${it.version}` });
    if (it.kind === "local" && it.origin === "dev")
      out.push({ key: "dev", cls: "tag soon", text: t("themes.origin.dev"), title: t("themes.origin.devHint") });
    if (it.id === activeId) out.push({ key: "active", cls: "tag ok", text: t("themes.inUse") });
    if (it.id === tryingId) out.push({ key: "trying", cls: "tag soon", text: t("themes.trying") });
    if (it.kind === "store" && it.installedVersion === it.version)
      out.push({ key: "inst", cls: "tag ok", text: t("themes.online.installed") });
    if (it.codexVerified)
      out.push({
        key: "cv",
        cls: "tag soon",
        text: `@${it.codexVerified.split(".").slice(0, 2).join(".")}`,
        title: t("themes.verifiedHint", { v: it.codexVerified }),
      });
    return out;
  };

  const primary = (it: Item): { label: string; run: () => void } | null => {
    if (it.kind === "store") {
      const upToDate = it.installedVersion === it.version;
      if (upToDate) return null;
      const isUpgrade = it.installedVersion != null && it.installedVersion !== it.version;
      const key = `online:${it.id}`;
      return {
        label:
          busy === key
            ? t("themes.online.installing")
            : isUpgrade
              ? t("themes.online.update", { v: it.version ?? "" })
              : t("themes.online.install"),
        run: () => installOnline(it),
      };
    }
    if (it.id === activeId || it.id === tryingId) return null;
    const key = status?.cdpReady ? `tryon:${it.id}` : `tryon-restart:${it.id}`;
    return {
      label:
        busy === key
          ? t("themes.busy.tryOn")
          : status?.cdpReady
            ? t("themes.tryOn")
            : t("themes.tryOnRestart"),
      run: () => tryOn(it),
    };
  };

  const actionsFor = (it: Item, size: "sm") => {
    const p = primary(it);
    return (
      <>
        {p ? (
          <button
            className={`btn primary ${size}`}
            disabled={busy !== null || (it.kind === "local" && !status?.supported)}
            onClick={p.run}
          >
            {p.label}
          </button>
        ) : null}
        <button
          className={`btn ghost ${size} icon-only`}
          onClick={() => setDetail(it)}
          aria-label={t("themes.details")}
          title={t("themes.details")}
        >
          <Icon name="info" />
        </button>
        {deletable(it) ? (
          <button
            className={`btn ghost ${size} icon-only danger`}
            disabled={busy !== null}
            onClick={() => setConfirmIds([it.id])}
            aria-label={t("themes.delete")}
            title={t("themes.delete")}
          >
            <Icon name="trash" />
          </button>
        ) : null}
      </>
    );
  };

  const selBox = (it: Item) =>
    selecting && deletable(it) ? (
      <label className="skin-check" title={t("themes.select")}>
        <input
          type="checkbox"
          checked={selected.has(it.id)}
          onChange={() => toggleSelect(it.id)}
          aria-label={t("themes.select")}
        />
        <span aria-hidden="true">
          <Icon name="check" />
        </span>
      </label>
    ) : null;

  const cover = (it: Item, className?: string) =>
    it.hasPreview ? (
      <PhotoCover
        load={it.loadPreview}
        onZoom={setLightbox}
        zoomLabel={t("themes.zoom")}
        className={className ?? "themecard-art themecard-art-photo"}
      />
    ) : (
      <ThemeCardArt colors={it.colors} />
    );

  const renderCard = (it: Item) => (
    <article
      key={`${it.kind}:${it.id}`}
      className={`themecard${it.id === activeId ? " active" : ""}${selected.has(it.id) ? " selected" : ""}`}
    >
      {selBox(it)}
      {cover(it)}
      <div className="themecard-body">
        <div className="themecard-head">
          <span className="themecard-name">{it.name}</span>
          {badges(it).map((b) => (
            <span key={b.key} className={b.cls === "version" ? "themecard-version" : b.cls} title={b.title}>
              {b.text}
            </span>
          ))}
        </div>
        {it.author ? <span className="themecard-author">@{it.author}</span> : null}
        {it.description ? <p className="themecard-desc">{it.description}</p> : null}
        {Object.keys(it.colors).length ? (
          <div className="themecard-swatches" aria-hidden="true">
            {Object.entries(it.colors)
              .slice(0, 10)
              .map(([key, value]) => (
                <span key={key} className="swatch" style={{ background: value }} title={key} />
              ))}
          </div>
        ) : null}
        <div className="themecard-actions">{actionsFor(it, "sm")}</div>
      </div>
    </article>
  );

  const renderRow = (it: Item) => (
    <div
      key={`${it.kind}:${it.id}`}
      className={`skinrow${it.id === activeId ? " active" : ""}${selected.has(it.id) ? " selected" : ""}`}
    >
      {selBox(it)}
      <div className="skinrow-thumb">{cover(it, "skinrow-cover")}</div>
      <div className="skinrow-main">
        <div className="skinrow-head">
          <span className="skinrow-name">{it.name}</span>
          {badges(it).map((b) => (
            <span key={b.key} className={b.cls === "version" ? "themecard-version" : b.cls} title={b.title}>
              {b.text}
            </span>
          ))}
        </div>
        <div className="skinrow-sub">
          {it.author ? <span className="themecard-author">@{it.author}</span> : null}
          {it.description ? <span className="skinrow-desc">{it.description}</span> : null}
        </div>
      </div>
      <div className="skinrow-actions">{actionsFor(it, "sm")}</div>
    </div>
  );

  const empty = tab === "local" ? loaded && items.length === 0 : catalog !== null && items.length === 0;

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
                  onClick={() => void run(`apply:${activeId}`, () => managerApi.codexThemeApply(activeId))}
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
            onChange={(key) => {
              setTab(key as GalleryTab);
              exitSelect();
            }}
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
          <div className="view-toggle" role="group" aria-label={t("themes.view")}>
            <button
              className={`iconbtn${view === "card" ? " active" : ""}`}
              aria-pressed={view === "card"}
              title={t("themes.view.card")}
              onClick={() => setViewMode("card")}
            >
              <Icon name="grid" />
            </button>
            <button
              className={`iconbtn${view === "list" ? " active" : ""}`}
              aria-pressed={view === "list"}
              title={t("themes.view.list")}
              onClick={() => setViewMode("list")}
            >
              <Icon name="list" />
            </button>
          </div>
          {tab === "local" ? (
            <button
              className={`btn ghost sm${selecting ? " active" : ""}`}
              onClick={() => (selecting ? exitSelect() : setSelecting(true))}
            >
              {selecting ? t("themes.select.done") : t("themes.select.manage")}
            </button>
          ) : null}
        </div>

        {selecting && tab === "local" ? (
          <div className="select-bar">
            <label className="skin-check inline">
              <input
                type="checkbox"
                checked={allSelected}
                onChange={toggleSelectAll}
                aria-label={t("themes.select.all")}
              />
              <span aria-hidden="true">
                <Icon name="check" />
              </span>
            </label>
            <span className="select-count">{t("themes.select.count", { n: String(selected.size) })}</span>
            <span className="row-actions" style={{ marginInlineStart: "auto" }}>
              <button
                className="btn danger sm"
                disabled={busy !== null || selected.size === 0}
                onClick={() => setConfirmIds([...selected])}
              >
                {t("themes.select.delete", { n: String(selected.size) })}
              </button>
              <button className="btn ghost sm" onClick={exitSelect}>
                {t("themes.select.cancel")}
              </button>
            </span>
          </div>
        ) : null}

        {tab === "store" && catalogFailed ? (
          <StatusBanner tone="info">{t("themes.online.offline")}</StatusBanner>
        ) : null}
        {tab === "store" && !catalogFailed && catalog === null ? (
          <p className="themes-noresult">{t("themes.online.loading")}</p>
        ) : null}

        {view === "card" ? (
          <div className="themegrid">{paged.map(renderCard)}</div>
        ) : (
          <div className="skinlist">{paged.map(renderRow)}</div>
        )}
        <Pagination page={clampedPage} pages={pages} onPage={setPage} label={t} />

        {empty ? (
          <section className="hero" style={{ paddingTop: 24 }}>
            <Icon name="sliders" className="ricon" />
            <div className="headline" style={{ fontSize: 16 }}>
              {t("themes.empty.title")}
            </div>
            <div className="desc">{t("themes.empty.sub")}</div>
          </section>
        ) : null}
        {!empty && loaded && visible.length === 0 ? (
          <p className="themes-noresult">{t("themes.noResults")}</p>
        ) : null}

        {tab === "local" ? (
          <>
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
                    onClick={() =>
                      void run("store", async () => {
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
                      })
                    }
                  >
                    {t("themes.storage.change")}
                  </button>
                  <button className="btn ghost sm" onClick={() => void managerApi.codexThemeOpenStore()}>
                    {t("themes.storage.open")}
                  </button>
                </span>
              </div>
              {storeNote ? (
                <div className="row">
                  <span className="rsub" role="status">
                    {storeNote}
                  </span>
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
        ) : null}
      </div>

      <DetailsSheet
        item={detail}
        onClose={() => setDetail(null)}
        onZoom={setLightbox}
        busy={busy}
        deletable={detail ? deletable(detail) : false}
        primary={detail ? primary(detail) : null}
        onDelete={(id) => setConfirmIds([id])}
        t={t}
      />
      <ConfirmDelete
        ids={confirmIds}
        names={(confirmIds ?? []).map((id) => themeName(id))}
        busy={busy === "delete"}
        onCancel={() => setConfirmIds(null)}
        onConfirm={() => confirmIds && doDelete(confirmIds)}
        t={t}
      />
      <Lightbox src={lightbox} onClose={() => setLightbox(null)} />
    </div>
  );
}

/** Full skin details, opened from a card/row info button. */
function DetailsSheet({
  item,
  onClose,
  onZoom,
  busy,
  deletable,
  primary,
  onDelete,
  t,
}: {
  item: Item | null;
  onClose: () => void;
  onZoom: (url: string) => void;
  busy: string | null;
  deletable: boolean;
  primary: { label: string; run: () => void } | null;
  onDelete: (id: string) => void;
  t: TFn;
}) {
  const rows = item
    ? ([
        ["themes.detail.id", item.id],
        ["themes.detail.version", item.version ? `v${item.version}` : "—"],
        ["themes.detail.author", item.author ? `@${item.author}` : "—"],
        ["themes.detail.appearance", item.appearance ?? "—"],
        ["themes.detail.verified", item.codexVerified ?? "—"],
        ["themes.detail.license", item.license ?? "—"],
        ["themes.detail.tags", item.tags.length ? item.tags.join(", ") : "—"],
      ] as const)
    : [];
  return (
    <Sheet open={item !== null} onDismiss={onClose} labelledBy="skin-detail-title">
      {item ? (
        <div className="skin-detail">
          <div className="skin-detail-cover">
            {item.hasPreview ? (
              <PhotoCover
                load={item.loadPreview}
                onZoom={onZoom}
                zoomLabel={t("themes.zoom")}
                className="skin-detail-photo"
              />
            ) : (
              <ThemeCardArt colors={item.colors} />
            )}
          </div>
          <h2 className="skin-detail-name" id="skin-detail-title">
            {item.name}
          </h2>
          {item.description ? <p className="skin-detail-desc">{item.description}</p> : null}
          <dl className="skin-detail-meta">
            {rows.map(([k, v]) => (
              <div className="skin-detail-row" key={k}>
                <dt>{t(k)}</dt>
                <dd className={k === "themes.detail.id" ? "mono" : undefined}>{v}</dd>
              </div>
            ))}
          </dl>
          {Object.keys(item.colors).length ? (
            <div className="themecard-swatches" aria-hidden="true">
              {Object.entries(item.colors).map(([key, value]) => (
                <span key={key} className="swatch" style={{ background: value }} title={key} />
              ))}
            </div>
          ) : null}
          <div className="sheet-actions row-actions">
            {primary ? (
              <button className="btn primary" disabled={busy !== null} onClick={primary.run}>
                {primary.label}
              </button>
            ) : null}
            {deletable ? (
              <button
                className="btn ghost danger"
                disabled={busy !== null}
                onClick={() => onDelete(item.id)}
              >
                {t("themes.delete")}
              </button>
            ) : null}
            <button className="btn ghost" onClick={onClose} style={{ marginInlineStart: "auto" }}>
              {t("themes.detail.close")}
            </button>
          </div>
        </div>
      ) : (
        <div />
      )}
    </Sheet>
  );
}

/** Delete confirmation for one skin or a batch. */
function ConfirmDelete({
  ids,
  names,
  busy,
  onCancel,
  onConfirm,
  t,
}: {
  ids: string[] | null;
  names: string[];
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
  t: TFn;
}) {
  const count = ids?.length ?? 0;
  return (
    <Sheet
      open={ids !== null}
      onDismiss={busy ? undefined : onCancel}
      dismissable={!busy}
      labelledBy="skin-del-title"
      initialFocus="dismiss"
    >
      <div className="confirm">
        <h2 className="confirm-title" id="skin-del-title">
          {count > 1 ? t("themes.delete.confirmMany", { n: String(count) }) : t("themes.delete.confirmOne")}
        </h2>
        <p className="confirm-body">
          {count > 1
            ? t("themes.delete.bodyMany", { n: String(count) })
            : t("themes.delete.bodyOne", { name: names[0] ?? "" })}
        </p>
        {count > 1 ? (
          <ul className="confirm-list">
            {names.slice(0, 8).map((n, i) => (
              <li key={i}>{n}</li>
            ))}
            {names.length > 8 ? <li>… {t("themes.delete.more", { n: String(names.length - 8) })}</li> : null}
          </ul>
        ) : null}
        <div className="sheet-actions row-actions">
          <button className="btn ghost" disabled={busy} onClick={onCancel}>
            {t("themes.delete.cancel")}
          </button>
          <button className="btn danger" disabled={busy} onClick={onConfirm}>
            {busy ? t("themes.delete.deleting") : t("themes.delete.ok")}
          </button>
        </div>
      </div>
    </Sheet>
  );
}
