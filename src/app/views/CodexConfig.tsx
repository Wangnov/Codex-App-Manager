import { useCallback, useEffect, useId, useRef, useState } from "react";

import { managerApi } from "../../services/managerApi";
import type { CodexFileSnapshot, CodexFileWhich } from "../../shared/types";
import { messageFailure, resolveFailure, type FailureSurface } from "../errorCopy";
import { Icon } from "../icons";
import { useI18n, type TFn } from "../i18n";
import { FailureBanner, NavBar, Segmented, StatusBanner } from "../components";

type Tab = CodexFileWhich;

/** Keep in sync with `MAX_BYTES` in `codex_files.rs`. */
const MAX_BYTES = 512 * 1024;

const EMPTY: CodexFileSnapshot = {
  which: "auth",
  path: "",
  content: "",
  exists: false,
  bytes: 0,
};

/**
 * Config-editor failures are mostly `internal_error` with an already
 * user-facing backend message (oversize, bad JSON, permission). Prefer that
 * text as the banner primary instead of a generic "something went wrong".
 */
function codexEditorFailure(cause: unknown, t: TFn): FailureSurface {
  const failure = resolveFailure(cause, t);
  if (
    failure.detail &&
    (failure.code === "internal_error" ||
      failure.code === "engine_error" ||
      failure.code === "unknown")
  ) {
    return { ...failure, message: failure.detail, detail: null };
  }
  return failure;
}

function clientValidate(which: Tab, content: string): string | null {
  if (content.length > MAX_BYTES) return "tooLarge";
  if (content.includes("\0")) return "nullByte";
  if (which !== "auth") return null;
  const trimmed = content.trim();
  if (!trimmed) return null;
  try {
    const value = JSON.parse(trimmed) as unknown;
    if (value === null || typeof value !== "object" || Array.isArray(value)) {
      return "object";
    }
  } catch {
    return "json";
  }
  return null;
}

export function CodexConfig({ onBack }: { onBack: () => void }) {
  const { t } = useI18n();
  const [tab, setTab] = useState<Tab>("auth");
  const [draft, setDraft] = useState("");
  const [saved, setSaved] = useState<CodexFileSnapshot>(EMPTY);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState<FailureSurface | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const editorId = useId();
  const pathId = useId();
  /** Bumped on every load; stale responses must not touch draft/saved. */
  const loadGen = useRef(0);

  const dirty = draft !== saved.content;
  const busy = loading || saving;

  const load = useCallback(
    async (which: Tab) => {
      const gen = ++loadGen.current;
      setLoading(true);
      setFailure(null);
      setNotice(null);
      try {
        const snap = await managerApi.readCodexFile(which);
        if (gen !== loadGen.current) return;
        setSaved(snap);
        setDraft(snap.content);
      } catch (cause) {
        if (gen !== loadGen.current) return;
        setFailure(codexEditorFailure(cause, t));
        setSaved({ ...EMPTY, which });
        setDraft("");
      } finally {
        if (gen === loadGen.current) {
          setLoading(false);
        }
      }
    },
    [t],
  );

  useEffect(() => {
    void load(tab);
  }, [tab, load]);

  const confirmDiscardIfDirty = (): boolean => {
    if (!dirty) return true;
    return window.confirm(t("config.discardConfirm"));
  };

  const handleBack = () => {
    if (!confirmDiscardIfDirty()) return;
    onBack();
  };

  const switchTab = (next: Tab) => {
    if (next === tab || busy) return;
    if (!confirmDiscardIfDirty()) return;
    setTab(next);
  };

  const save = async () => {
    const invalid = clientValidate(tab, draft);
    if (invalid === "json") {
      setFailure(messageFailure(t("config.invalidJson"), "invalid_json"));
      setNotice(null);
      return;
    }
    if (invalid === "object") {
      setFailure(messageFailure(t("config.invalidJsonObject"), "invalid_json"));
      setNotice(null);
      return;
    }
    if (invalid === "tooLarge") {
      setFailure(messageFailure(t("config.tooLarge"), "too_large"));
      setNotice(null);
      return;
    }
    if (invalid === "nullByte") {
      setFailure(messageFailure(t("config.nullByte"), "null_byte"));
      setNotice(null);
      return;
    }
    setSaving(true);
    setFailure(null);
    setNotice(null);
    try {
      const snap = await managerApi.writeCodexFile(tab, draft);
      setSaved(snap);
      setDraft(snap.content);
      setNotice(t("config.saved"));
    } catch (cause) {
      setFailure(codexEditorFailure(cause, t));
    } finally {
      setSaving(false);
    }
  };

  const reload = () => {
    if (busy) return;
    if (!confirmDiscardIfDirty()) return;
    void load(tab);
  };

  const openHome = async () => {
    setFailure(null);
    try {
      await managerApi.openCodexHome();
    } catch (cause) {
      setFailure(codexEditorFailure(cause, t));
    }
  };

  const placeholder =
    tab === "auth" ? t("config.placeholder.auth") : t("config.placeholder.config");

  return (
    <div className="pop">
      <NavBar title={t("nav.config")} onBack={handleBack} disableBack={saving} />
      <div className="scroll view codex-config">
        <p className="config-lead">{t("config.desc")}</p>
        <p className="config-note">{t("config.note.sub2api")}</p>

        <div className="config-tabs" aria-busy={loading || undefined}>
          <Segmented
            value={tab}
            onChange={(v) => switchTab(v as Tab)}
            ariaLabel={t("nav.config")}
            items={[
              { key: "auth", label: t("config.tab.auth") },
              { key: "config", label: t("config.tab.config") },
            ]}
          />
        </div>

        {failure ? <FailureBanner failure={failure} /> : null}
        {notice ? <StatusBanner tone="ok">{notice}</StatusBanner> : null}

        <div className="config-path-row">
          <span className="config-path-label" id={pathId}>
            {t("config.path")}
          </span>
          <span className="config-path mono" aria-labelledby={pathId}>
            {loading ? "…" : saved.path || "—"}
          </span>
          {!loading && !saved.exists ? (
            <span className="tag">{t("config.missing")}</span>
          ) : null}
          {dirty ? <span className="tag soon">{t("config.dirty")}</span> : null}
        </div>

        <label className="config-editor-label" htmlFor={editorId}>
          {tab === "auth" ? t("config.auth.title") : t("config.config.title")}
        </label>
        <p className="config-editor-hint">
          {tab === "auth" ? t("config.auth.hint") : t("config.config.hint")}
        </p>
        <textarea
          id={editorId}
          className="input mono config-editor"
          spellCheck={false}
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          disabled={busy}
          value={draft}
          placeholder={loading ? t("config.loading") : placeholder}
          onChange={(e) => {
            setDraft(e.target.value);
            setNotice(null);
            setFailure(null);
          }}
        />

        <div className="config-actions">
          <button
            type="button"
            className="btn primary"
            disabled={busy || !dirty}
            onClick={() => void save()}
          >
            {saving ? t("config.saving") : t("config.save")}
          </button>
          <button
            type="button"
            className="btn ghost"
            disabled={busy}
            onClick={reload}
          >
            <Icon name="refresh" />
            {t("config.reload")}
          </button>
          <button
            type="button"
            className="btn ghost"
            disabled={saving}
            onClick={() => void openHome()}
          >
            {t("config.openHome")}
          </button>
        </div>
      </div>
    </div>
  );
}
