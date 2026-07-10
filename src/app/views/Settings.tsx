import { useCallback, useEffect, useId, useState } from "react";

import { errorMessage, managerApi } from "../../services/managerApi";
import type {
  ConfigHealth,
  ConfigWhich,
  ProxyMode,
  UpdateSourceKind,
  WindowsInstallMode,
} from "../../shared/types";
import { DEFAULT_SETTINGS } from "../../shared/types";
import { Icon } from "../icons";
import { useI18n, LANGS, type Lang, type TFn, type TKey } from "../i18n";
import { useTheme, type ThemeMode } from "../theme";
import { NavBar, Segmented, Toggle, radioNavTarget } from "../components";
import { isWindows } from "../platform";
import { samePath } from "../paths";
import { Sheet } from "../Sheet";
import { useSettingsSaver } from "./useSettingsSaver";

const OK_HEALTH: ConfigHealth = {
  settingsStatus: "ok",
  provenanceStatus: "ok",
  unknownSource: null,
  detail: null,
  settingsBackupAvailable: false,
  provenanceBackupAvailable: false,
};

function healthLabel(status: string, t: TFn): string {
  if (status === "recovered") return t("settings.health.recovered");
  if (status === "corrupt") return t("settings.health.corrupt");
  return t("settings.health.ok");
}

function healthNeedsAttention(h: ConfigHealth): boolean {
  return (
    h.settingsStatus !== "ok" ||
    h.provenanceStatus !== "ok" ||
    Boolean(h.unknownSource)
  );
}

const SOURCES: { kind: UpdateSourceKind; label: TKey; desc: TKey | "" }[] = [
  { kind: "auto", label: "settings.source.auto", desc: "settings.source.autoDesc" },
  { kind: "mirror", label: "settings.source.mirror", desc: "settings.source.mirrorDesc" },
  { kind: "official", label: "settings.source.official", desc: "settings.source.officialDesc" },
  { kind: "custom", label: "settings.source.custom", desc: "" },
];

const WINDOWS_INSTALL_MODES: { kind: WindowsInstallMode; label: TKey; desc: TKey }[] = [
  {
    kind: "msix",
    label: "settings.windows.msix",
    desc: "settings.windows.msixDesc",
  },
  {
    kind: "portable",
    label: "settings.windows.portable",
    desc: "settings.windows.portableDescDefault",
  },
];

const FREQUENCY_PRESETS: { seconds: number; label: TKey }[] = [
  { seconds: 15 * 60, label: "settings.general.interval15m" },
  { seconds: 60 * 60, label: "settings.general.interval1h" },
  { seconds: 6 * 60 * 60, label: "settings.general.interval6h" },
];

const PROXY_MODES: { kind: ProxyMode; label: TKey }[] = [
  { kind: "system", label: "settings.network.proxySystem" },
  { kind: "direct", label: "settings.network.proxyDirect" },
  { kind: "custom", label: "settings.network.proxyCustom" },
];

const MIN_CUSTOM_INTERVAL_SECONDS = 60;
const MAX_CUSTOM_HOURS = 168;

function splitInterval(totalSeconds: number) {
  const total = Math.max(MIN_CUSTOM_INTERVAL_SECONDS, Math.floor(totalSeconds || 0));
  return {
    hours: Math.floor(total / 3600),
    minutes: Math.floor((total % 3600) / 60),
    seconds: total % 60,
  };
}

function intervalFromParts(parts: { hours: number; minutes: number; seconds: number }) {
  return Math.max(
    MIN_CUSTOM_INTERVAL_SECONDS,
    parts.hours * 3600 + parts.minutes * 60 + parts.seconds,
  );
}

function clampPart(value: string, max: number) {
  const n = Math.floor(Number(value));
  if (!Number.isFinite(n)) return 0;
  return Math.max(0, Math.min(max, n));
}

function formatInterval(seconds: number, t: TFn) {
  const preset = FREQUENCY_PRESETS.find((option) => option.seconds === seconds);
  if (preset) return t(preset.label);
  const parts = splitInterval(seconds);
  const labels = [
    parts.hours ? `${parts.hours}${t("settings.general.intervalHoursSuffix")}` : "",
    parts.minutes ? `${parts.minutes}${t("settings.general.intervalMinutesSuffix")}` : "",
    parts.seconds ? `${parts.seconds}${t("settings.general.intervalSecondsSuffix")}` : "",
  ].filter(Boolean);
  return labels.join(" ") || `1${t("settings.general.intervalMinutesSuffix")}`;
}

export function Settings({
  onBack,
  onOpenAbout,
  onOpenUninstall,
  onOpenConfig: _onOpenConfig,
}: {
  onBack: () => void;
  onOpenAbout: () => void;
  onOpenUninstall: () => void;
  /** Reserved: Codex config is not shippable yet; entry stays disabled. */
  onOpenConfig: () => void;
}) {
  const { t, lang, setLang } = useI18n();
  const { mode, setMode } = useTheme();
  const win = isWindows();
  const {
    settings: s,
    status: saveStatus,
    error: saveError,
    hydrated,
    update,
    retry,
    hydrate,
    reset,
    setDraft,
  } = useSettingsSaver(DEFAULT_SETTINGS);
  const [defaultInstallRoot, setDefaultInstallRoot] = useState(DEFAULT_SETTINGS.installRoot);
  const [autostart, setAutostart] = useState(false);
  const [commandError, setCommandError] = useState<string | null>(null);
  const [fieldError, setFieldError] = useState<string | null>(null);
  const [langSheet, setLangSheet] = useState(false);
  const [customIntervalOpen, setCustomIntervalOpen] = useState(false);
  const [configHealth, setConfigHealth] = useState<ConfigHealth>(OK_HEALTH);
  const [healthBusy, setHealthBusy] = useState<string | null>(null);
  const [healthNotice, setHealthNotice] = useState<string | null>(null);
  const [healthConfirm, setHealthConfirm] = useState<
    null | { kind: "restore" | "reset"; which: ConfigWhich }
  >(null);
  const langTitleId = useId();
  const healthConfirmTitleId = useId();
  const healthConfirmBodyId = useId();
  // Prefix for the switch-row title ids — each Toggle names itself off its
  // visible row title via aria-labelledby.
  const switchId = useId();

  const refreshHealth = useCallback(async () => {
    try {
      setConfigHealth(await managerApi.getConfigHealth());
    } catch {
      // Leave last-known health; surface via commandError only on explicit actions.
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    void managerApi
      .getSettings()
      .then((settings) => {
        if (!cancelled) hydrate(settings);
      })
      .catch(() => {
        if (!cancelled) hydrate(DEFAULT_SETTINGS);
      });
    void managerApi.getAutostart().then(setAutostart).catch(() => undefined);
    void refreshHealth();
    if (win) {
      void managerApi.winDefaultInstallRoot().then(setDefaultInstallRoot).catch(() => undefined);
    }
    return () => {
      cancelled = true;
    };
  }, [hydrate, win, refreshHealth]);

  const runHealthAction = async (kind: "restore" | "reset", which: ConfigWhich) => {
    setHealthConfirm(null);
    setHealthBusy(`${kind}:${which}`);
    setCommandError(null);
    setHealthNotice(null);
    try {
      const next =
        kind === "restore"
          ? await managerApi.restoreConfigBackup(which)
          : await managerApi.resetConfig(which);
      setConfigHealth(next);
      // Re-read verified health + reload settings so the form matches disk.
      const verified = await managerApi.getConfigHealth();
      setConfigHealth(verified);
      try {
        reset(await managerApi.getSettings());
      } catch {
        // Settings reload is best-effort after provenance-only ops.
      }
      setHealthNotice(t("settings.health.verified"));
    } catch (cause) {
      setCommandError(errorMessage(cause));
      await refreshHealth();
    } finally {
      setHealthBusy(null);
    }
  };

  const pickInstallRoot = async () => {
    setCommandError(null);
    try {
      const path = await managerApi.winPickInstallDir();
      if (!path) return;
      reset(await managerApi.winSetInstallRoot(path));
    } catch (cause) {
      setCommandError(errorMessage(cause));
    }
  };

  const resetInstallRoot = async () => {
    setCommandError(null);
    try {
      reset(await managerApi.winResetInstallRoot());
    } catch (cause) {
      setCommandError(errorMessage(cause));
    }
  };

  const toggleAutostart = (v: boolean) => {
    setAutostart(v);
    // Revert the UI if the OS write fails so the toggle never lies.
    void managerApi.setAutostart(v).catch(() => setAutostart(!v));
  };

  const restoreSkippedUpdate = () => {
    setCommandError(null);
    setFieldError(null);
    update({ ...s, skippedCodexUpdate: null });
  };

  const selectSource = (next: UpdateSourceKind) => {
    setCommandError(null);
    setFieldError(null);
    const draft = { ...s, source: next };
    // Custom with no URL is a draft only — not saved until a value exists.
    if (next === "custom" && !s.customUrl.trim()) {
      setDraft(draft);
      setFieldError(t("settings.source.customRequired"));
      return;
    }
    update(draft);
  };

  const selectProxyMode = (key: ProxyMode) => {
    setCommandError(null);
    setFieldError(null);
    const next = { ...s, proxyMode: key };
    if (key === "custom" && !s.customProxyUrl.trim()) {
      setDraft(next);
      setFieldError(t("settings.network.proxyRequired"));
      return;
    }
    update(next);
  };

  const themes: { v: ThemeMode; k: "settings.appearance.system" | "settings.appearance.light" | "settings.appearance.dark" }[] = [
    { v: "system", k: "settings.appearance.system" },
    { v: "light", k: "settings.appearance.light" },
    { v: "dark", k: "settings.appearance.dark" },
  ];
  const installRootIsDefault = samePath(s.installRoot, defaultInstallRoot);
  const portableDescKey: TKey = installRootIsDefault
    ? "settings.windows.portableDescDefault"
    : "settings.windows.portableDescCustom";
  const availableSources = SOURCES.filter((src) => !(win && src.kind === "official"));
  const showingSaveError = !commandError && Boolean(saveError);
  const error = commandError ?? saveError ?? fieldError;
  const skippedUpdate = s.skippedCodexUpdate;
  const customInterval = customIntervalOpen || !FREQUENCY_PRESETS.some(
    (option) => option.seconds === s.periodicCheckIntervalSeconds,
  );
  const intervalParts = splitInterval(s.periodicCheckIntervalSeconds);
  const intervalLabel = formatInterval(s.periodicCheckIntervalSeconds, t);
  const formLocked = !hydrated;

  const updateIntervalPart = (
    part: "hours" | "minutes" | "seconds",
    value: string,
  ) => {
    const nextParts = { ...splitInterval(s.periodicCheckIntervalSeconds) };
    nextParts[part] = clampPart(value, part === "hours" ? MAX_CUSTOM_HOURS : 59);
    setCommandError(null);
    setFieldError(null);
    update({ ...s, periodicCheckIntervalSeconds: intervalFromParts(nextParts) });
  };

  const whichLabel = (which: ConfigWhich) =>
    which === "settings" ? t("settings.health.settings") : t("settings.health.provenance");

  return (
    <div className="pop">
      <NavBar title={t("settings.title")} onBack={onBack} />
      <div
        className="scroll view"
        inert={formLocked || langSheet || healthConfirm ? true : undefined}
      >
        {!hydrated ? (
          <div className="banner info" role="status" aria-live="polite">
            <Icon name="loader" />
            <span>{t("settings.loading")}</span>
          </div>
        ) : null}
        {hydrated && saveStatus === "saving" ? (
          <div className="banner info" role="status">
            <Icon name="loader" />
            <span>{t("settings.saving")}</span>
          </div>
        ) : null}
        {error ? (
          <div className="banner err" role="alert">
            <Icon name="alert" />
            <span>
              {showingSaveError ? `${t("settings.saveError")}: ${error}` : error}
            </span>
            {showingSaveError ? (
              <button className="linkbtn" onClick={retry}>
                {t("settings.retry")}
              </button>
            ) : null}
          </div>
        ) : null}
        {healthNeedsAttention(configHealth) ? (
          <div className="banner warn" role="status">
            <Icon name="alert" />
            <span>{t("settings.health.banner")}</span>
          </div>
        ) : null}
        {healthNotice ? (
          <div className="banner info" role="status">
            <Icon name="check" />
            <span>{healthNotice}</span>
          </div>
        ) : null}

        {/* 配置健康 — persistent, with restore/reset consequence flows */}
        <div className="group">
          <div className="group-h">{t("settings.health.header")}</div>
          <div className="list">
            {(
              [
                {
                  which: "settings" as const,
                  status: configHealth.settingsStatus,
                  backup: configHealth.settingsBackupAvailable,
                },
                {
                  which: "provenance" as const,
                  status: configHealth.provenanceStatus,
                  backup: configHealth.provenanceBackupAvailable,
                },
              ] as const
            ).map((row) => (
              <div key={row.which} className="row" style={{ display: "block" }}>
                <div className="rtext" style={{ marginBottom: 8 }}>
                  <span className="rtitle">
                    {whichLabel(row.which)}
                    <span className="tag" style={{ marginInlineStart: 8 }}>
                      {healthLabel(row.status, t)}
                    </span>
                  </span>
                  {!row.backup ? (
                    <span className="rsub">{t("settings.health.noBackup")}</span>
                  ) : null}
                </div>
                <div className="install-root-actions">
                  <button
                    className="mini-action"
                    disabled={Boolean(healthBusy) || !row.backup}
                    onClick={() => setHealthConfirm({ kind: "restore", which: row.which })}
                  >
                    {healthBusy === `restore:${row.which}`
                      ? t("settings.health.working")
                      : t("settings.health.restore")}
                  </button>
                  <button
                    className="mini-action"
                    disabled={Boolean(healthBusy)}
                    onClick={() => setHealthConfirm({ kind: "reset", which: row.which })}
                  >
                    {healthBusy === `reset:${row.which}`
                      ? t("settings.health.working")
                      : t("settings.health.reset")}
                  </button>
                </div>
              </div>
            ))}
            {configHealth.unknownSource ? (
              <div className="row">
                <span className="rtext">
                  <span className="rtitle">{t("settings.health.unknownSource")}</span>
                  <span className="rsub mono">{configHealth.unknownSource}</span>
                </span>
              </div>
            ) : null}
            {configHealth.detail ? (
              <div className="row">
                <span className="rtext">
                  <span className="rtitle">{t("settings.health.detail")}</span>
                  <span className="rsub">{configHealth.detail}</span>
                </span>
              </div>
            ) : null}
          </div>
        </div>

        {/* 更新源 */}
        <div className="group">
          <div className="group-h">{t("settings.source.header")}</div>
          {/* Roving-tabindex radiogroup — the child radios carry focus + Tab, so
              the group is correctly non-focusable (WAI-ARIA APG). */}
          {/* eslint-disable-next-line jsx-a11y/interactive-supports-focus */}
          <div
            className="list"
            role="radiogroup"
            aria-label={t("settings.source.header")}
            onKeyDown={(event) => {
              const next = radioNavTarget(
                availableSources.map((src) => src.kind),
                s.source,
                event,
                event.currentTarget,
              );
              if (next === null) return;
              selectSource(next as UpdateSourceKind);
            }}
          >
            {availableSources.map((src) => (
              <button
                key={src.kind}
                className="row"
                role="radio"
                aria-checked={s.source === src.kind}
                tabIndex={s.source === src.kind ? 0 : -1}
                onClick={() => selectSource(src.kind)}
              >
                <span className="radio" />
                <span className="rtext">
                  <span className="rtitle">
                    {t(src.label)}
                    {src.kind === "auto" ? (
                      <span className="tag" style={{ marginInlineStart: 8 }}>
                        {t("settings.source.recommended")}
                      </span>
                    ) : null}
                  </span>
                  {src.desc ? <span className="rsub">{t(src.desc)}</span> : null}
                </span>
              </button>
            ))}
            {s.source === "custom" ? (
              <div className="row" style={{ display: "block" }}>
                <input
                  className="input mono"
                  aria-label={t("settings.source.custom")}
                  aria-invalid={Boolean(fieldError && !s.customUrl.trim()) || undefined}
                  value={s.customUrl}
                  placeholder={t("settings.source.customPlaceholder")}
                  onChange={(e) => {
                    setFieldError(null);
                    setDraft({ ...s, customUrl: e.target.value });
                  }}
                  onBlur={(e) => {
                    setCommandError(null);
                    // Read from the input — closed-over s.customUrl can lag the DOM value.
                    const customUrl = e.currentTarget.value;
                    if (!customUrl.trim()) {
                      // Abandon incomplete custom so UI selection matches runtime
                      // (empty custom must not leave the previous URL active).
                      setFieldError(null);
                      update({ ...s, source: "auto", customUrl: "" });
                      return;
                    }
                    setFieldError(null);
                    update({ ...s, customUrl });
                  }}
                />
                {!s.customUrl.trim() ? (
                  <span className="rsub" style={{ display: "block", marginTop: 8 }}>
                    {t("settings.source.customRequired")}
                  </span>
                ) : null}
              </div>
            ) : null}
          </div>
        </div>

        {win ? (
          <div className="group">
            <div className="group-h">{t("settings.windows.header")}</div>
            {/* Roving-tabindex radiogroup — child radios carry focus + Tab. */}
            {/* eslint-disable-next-line jsx-a11y/interactive-supports-focus */}
            <div
              className="list"
              role="radiogroup"
              aria-label={t("settings.windows.header")}
              onKeyDown={(event) => {
                const next = radioNavTarget(
                  WINDOWS_INSTALL_MODES.map((mode) => mode.kind),
                  s.windowsInstallMode,
                  event,
                  event.currentTarget,
                );
                if (next === null) return;
                setCommandError(null);
                setFieldError(null);
                update({ ...s, windowsInstallMode: next as WindowsInstallMode });
              }}
            >
              {WINDOWS_INSTALL_MODES.map((mode) => (
                <button
                  key={mode.kind}
                  className="row"
                  role="radio"
                  aria-checked={s.windowsInstallMode === mode.kind}
                  tabIndex={s.windowsInstallMode === mode.kind ? 0 : -1}
                  onClick={() => {
                    setCommandError(null);
                    setFieldError(null);
                    update({ ...s, windowsInstallMode: mode.kind });
                  }}
                >
                  <span className="radio" />
                  <span className="rtext">
                    <span className="rtitle">
                      {t(mode.label)}
                      {mode.kind === "msix" ? (
                        <span className="tag" style={{ marginInlineStart: 8 }}>
                          {t("settings.source.recommended")}
                        </span>
                      ) : null}
                    </span>
                    <span className="rsub">{t(mode.kind === "portable" ? portableDescKey : mode.desc)}</span>
                  </span>
                </button>
              ))}
              {s.windowsInstallMode === "portable" ? (
                <div className="install-root-row">
                  <div className="install-root-copy">
                    <Icon name="download" className="ricon" />
                    <span className="rtext">
                      <span className="rtitle">
                        {t("settings.windows.installRoot")}
                        <span className="tag" style={{ marginInlineStart: 8 }}>
                          {t(
                            installRootIsDefault
                              ? "settings.windows.installRootDefault"
                              : "settings.windows.installRootCustom",
                          )}
                        </span>
                      </span>
                      <span className="rsub">{t("settings.windows.installRootDesc")}</span>
                    </span>
                  </div>
                  <button className="install-root-path" onClick={pickInstallRoot}>
                    <span className="install-root-value mono">{s.installRoot}</span>
                    <Icon name="chevron" className="chev" />
                  </button>
                  <div className="install-root-actions">
                    <button
                      className="mini-action"
                      onClick={resetInstallRoot}
                      disabled={installRootIsDefault}
                    >
                      <Icon name="refresh" />
                      {t("settings.windows.installRootReset")}
                    </button>
                  </div>
                </div>
              ) : null}
            </div>
          </div>
        ) : null}

        <div className="group">
          <div className="group-h">{t("settings.updateReminder.header")}</div>
          <div className="list">
            {skippedUpdate ? (
              <>
                <div className="row">
                  <Icon name="info" className="ricon" />
                  <span className="rtext">
                    <span className="rtitle">{t("settings.updateReminder.skippedVersion")}</span>
                    <span className="rsub">{t("settings.updateReminder.quiet")}</span>
                  </span>
                  <span className="rval mono">{skippedUpdate.version}</span>
                </div>
                <div className="settings-row-actions">
                  <button className="mini-action" onClick={restoreSkippedUpdate}>
                    <Icon name="refresh" />
                    {t("settings.updateReminder.restore")}
                  </button>
                </div>
              </>
            ) : (
              <div className="row">
                <Icon name="info" className="ricon" />
                <span className="rtext">
                  <span className="rtitle">{t("settings.updateReminder.none")}</span>
                </span>
              </div>
            )}
          </div>
        </div>

        {/* 通用 */}
        <div className="group">
          <div className="group-h">{t("settings.general.header")}</div>
          <div className="list">
            <div className="row">
              <span className="rtext">
                <span className="rtitle" id={`${switchId}-startup`}>{t("settings.general.checkOnStartup")}</span>
              </span>
              <Toggle
                ariaLabelledBy={`${switchId}-startup`}
                checked={s.checkOnStartup}
                onChange={(v) => {
                  setCommandError(null);
                  setFieldError(null);
                  update({ ...s, checkOnStartup: v });
                }}
              />
            </div>
            <div className="row">
              <span className="rtext">
                <span className="rtitle" id={`${switchId}-periodic`}>{t("settings.general.periodicCheck")}</span>
                <span className="rsub">{t("settings.general.periodicCheckNote")}</span>
              </span>
              <Toggle
                ariaLabelledBy={`${switchId}-periodic`}
                checked={s.periodicCheck}
                onChange={(v) => {
                  setCommandError(null);
                  setFieldError(null);
                  update({ ...s, autoCheck: v, periodicCheck: v });
                }}
              />
            </div>
            <div
              className={`schedule-panel ${s.periodicCheck ? "open" : ""}`}
              aria-hidden={!s.periodicCheck}
              inert={s.periodicCheck ? undefined : true}
            >
              <div className="schedule-panel-inner">
                <div className="schedule-panel-content">
                  <div className="schedule-head">
                    <span className="rtitle">{t("settings.general.checkFrequency")}</span>
                    <span className="tag">{t("settings.general.intervalEvery", { interval: intervalLabel })}</span>
                  </div>
                  <Segmented
                    ariaLabel={t("settings.general.checkFrequency")}
                    value={customInterval ? "custom" : String(s.periodicCheckIntervalSeconds)}
                    items={[
                      ...FREQUENCY_PRESETS.map((option) => ({
                        key: String(option.seconds),
                        label: t(option.label),
                      })),
                      { key: "custom", label: t("settings.general.customInterval") },
                    ]}
                    onChange={(key) => {
                      setCommandError(null);
                      setFieldError(null);
                      if (key === "custom") {
                        setCustomIntervalOpen(true);
                        return;
                      }
                      setCustomIntervalOpen(false);
                      update({ ...s, periodicCheckIntervalSeconds: Number(key) });
                    }}
                  />
                  {customInterval ? (
                    <div className="interval-grid">
                      <label>
                        <span>{t("settings.general.intervalHours")}</span>
                        <input
                          className="input mono"
                          type="number"
                          inputMode="numeric"
                          min={0}
                          max={MAX_CUSTOM_HOURS}
                          value={intervalParts.hours}
                          onChange={(e) => updateIntervalPart("hours", e.target.value)}
                        />
                      </label>
                      <label>
                        <span>{t("settings.general.intervalMinutes")}</span>
                        <input
                          className="input mono"
                          type="number"
                          inputMode="numeric"
                          min={0}
                          max={59}
                          value={intervalParts.minutes}
                          onChange={(e) => updateIntervalPart("minutes", e.target.value)}
                        />
                      </label>
                      <label>
                        <span>{t("settings.general.intervalSeconds")}</span>
                        <input
                          className="input mono"
                          type="number"
                          inputMode="numeric"
                          min={0}
                          max={59}
                          value={intervalParts.seconds}
                          onChange={(e) => updateIntervalPart("seconds", e.target.value)}
                        />
                      </label>
                    </div>
                  ) : null}
                </div>
              </div>
            </div>
            <div className="row">
              <span className="rtext">
                <span className="rtitle" id={`${switchId}-ask`}>{t("settings.general.askBefore")}</span>
              </span>
              <Toggle
                ariaLabelledBy={`${switchId}-ask`}
                checked={s.askBefore}
                onChange={(v) => {
                  setCommandError(null);
                  setFieldError(null);
                  update({ ...s, askBefore: v });
                }}
              />
            </div>
            <div className="row">
              <span className="rtext">
                <span className="rtitle" id={`${switchId}-noselfupdate`}>{t("settings.general.disableCodexSelfUpdates")}</span>
              </span>
              <Toggle
                ariaLabelledBy={`${switchId}-noselfupdate`}
                checked={s.disableCodexSelfUpdates}
                onChange={(v) => {
                  setCommandError(null);
                  setFieldError(null);
                  update({ ...s, disableCodexSelfUpdates: v });
                }}
              />
            </div>
            <div className="row">
              <span className="rtext">
                <span className="rtitle" id={`${switchId}-autostart`}>{t("settings.general.autostart")}</span>
                <span className="rsub">{t("settings.general.autostartNote")}</span>
              </span>
              <Toggle
                ariaLabelledBy={`${switchId}-autostart`}
                checked={autostart}
                onChange={toggleAutostart}
              />
            </div>
            <div className="row">
              <span className="rtext">
                <span className="rtitle" id={`${switchId}-confirmclose`}>{t("settings.general.confirmClose")}</span>
              </span>
              <Toggle
                ariaLabelledBy={`${switchId}-confirmclose`}
                checked={s.confirmClose}
                onChange={(v) => {
                  setCommandError(null);
                  setFieldError(null);
                  update({ ...s, confirmClose: v });
                }}
              />
            </div>
          </div>
        </div>

        {/* 外观 */}
        <div className="group">
          <div className="group-h">{t("settings.appearance.header")}</div>
          <div className="list">
            <div className="row" style={{ display: "block" }}>
              <div className="rtitle" style={{ marginBottom: 8 }}>
                {t("settings.appearance.theme")}
              </div>
              <Segmented
                ariaLabel={t("settings.appearance.theme")}
                value={mode}
                items={themes.map((th) => ({ key: th.v, label: t(th.k) }))}
                onChange={(key) => setMode(key as ThemeMode)}
              />
            </div>
            <button className="row" onClick={() => setLangSheet(true)}>
              <span className="rtext">
                <span className="rtitle">{t("settings.appearance.language")}</span>
              </span>
              <span className="rval">{LANGS.find((l) => l.code === lang)?.native ?? lang}</span>
              <Icon name="chevron" className="chev" />
            </button>
          </div>
        </div>

        {/* 网络 */}
        <div className="group">
          <div className="group-h">{t("settings.network.header")}</div>
          <div className="list">
            <div className="row" style={{ display: "block" }}>
              <div className="rtitle" style={{ marginBottom: 8 }}>
                {t("settings.network.proxy")}
              </div>
              <Segmented
                ariaLabel={t("settings.network.proxy")}
                value={s.proxyMode}
                items={PROXY_MODES.map((mode) => ({ key: mode.kind, label: t(mode.label) }))}
                onChange={(key) => selectProxyMode(key as ProxyMode)}
              />
              {s.proxyMode === "system" ? (
                <span className="rsub" style={{ display: "block", marginTop: 8 }}>
                  {t("settings.network.proxySystemDesc")}
                </span>
              ) : null}
              {s.proxyMode === "custom" ? (
                <div style={{ marginTop: 10 }}>
                  <input
                    className="input mono"
                    aria-label={t("settings.network.proxyCustom")}
                    aria-invalid={Boolean(fieldError && !s.customProxyUrl.trim()) || undefined}
                    value={s.customProxyUrl}
                    placeholder={t("settings.network.proxyPlaceholder")}
                    onChange={(e) => {
                      setFieldError(null);
                      setDraft({ ...s, customProxyUrl: e.target.value });
                    }}
                    onBlur={(e) => {
                      setCommandError(null);
                      const customProxyUrl = e.currentTarget.value;
                      if (!customProxyUrl.trim()) {
                        // Same contract as custom source: empty blur coerces to a
                        // real mode so UI and runtime do not keep the old proxy URL.
                        setFieldError(null);
                        update({ ...s, proxyMode: "system", customProxyUrl: "" });
                        return;
                      }
                      setFieldError(null);
                      update({ ...s, customProxyUrl });
                    }}
                  />
                  {!s.customProxyUrl.trim() ? (
                    <span className="rsub" style={{ display: "block", marginTop: 8 }}>
                      {t("settings.network.proxyRequired")}
                    </span>
                  ) : null}
                </div>
              ) : null}
            </div>
          </div>
        </div>

        {/* 更多 */}
        <div className="group">
          <div className="group-h">{t("settings.more.header")}</div>
          <div className="list">
            <button
              className="row"
              type="button"
              disabled
              aria-disabled="true"
              title={t("settings.more.configUnavailable")}
            >
              <Icon name="sliders" className="ricon" />
              <span className="rtext">
                <span className="rtitle">{t("settings.more.config")}</span>
                <span className="rsub">{t("settings.more.configUnavailable")}</span>
              </span>
              <span className="tag soon">{t("settings.more.soon")}</span>
            </button>
            <button className="row" onClick={onOpenAbout}>
              <Icon name="info" className="ricon" />
              <span className="rtext">
                <span className="rtitle">{t("settings.more.about")}</span>
              </span>
              <Icon name="chevron" className="chev" />
            </button>
            <button className="row danger" onClick={onOpenUninstall}>
              <Icon name="trash" className="ricon" />
              <span className="rtext">
                <span className="rtitle">{t("settings.more.uninstall")}</span>
              </span>
              <Icon name="chevron" className="chev" />
            </button>
          </div>
        </div>
      </div>

      <Sheet
        open={healthConfirm != null}
        onDismiss={() => setHealthConfirm(null)}
        labelledBy={healthConfirmTitleId}
        describedBy={healthConfirmBodyId}
        initialFocus="dismiss"
      >
        {healthConfirm ? (
          <>
            <h3 id={healthConfirmTitleId}>
              {healthConfirm.kind === "restore"
                ? t("settings.health.restoreConfirm.title", {
                    which: whichLabel(healthConfirm.which),
                  })
                : t("settings.health.resetConfirm.title", {
                    which: whichLabel(healthConfirm.which),
                  })}
            </h3>
            <p id={healthConfirmBodyId}>
              {healthConfirm.kind === "restore"
                ? t("settings.health.restoreConfirm.body")
                : healthConfirm.which === "settings"
                  ? t("settings.health.resetConfirm.body.settings")
                  : t("settings.health.resetConfirm.body.provenance")}
            </p>
            <div className="row2">
              <button className="btn ghost" onClick={() => setHealthConfirm(null)}>
                {t("confirm.cancel")}
              </button>
              <button
                className="btn danger"
                onClick={() =>
                  void runHealthAction(healthConfirm.kind, healthConfirm.which)
                }
              >
                {healthConfirm.kind === "restore"
                  ? t("settings.health.restore")
                  : t("settings.health.reset")}
              </button>
            </div>
          </>
        ) : null}
      </Sheet>

      <Sheet
        open={langSheet}
        onDismiss={() => setLangSheet(false)}
        labelledBy={langTitleId}
        initialFocus="first"
      >
        <h3 id={langTitleId}>{t("settings.appearance.language")}</h3>
        {/* Roving-tabindex radiogroup — child radios carry focus + Tab. */}
        {/* eslint-disable-next-line jsx-a11y/interactive-supports-focus */}
        <div
          className="langgrid"
          style={{ marginTop: 14 }}
          role="radiogroup"
          aria-labelledby={langTitleId}
          onKeyDown={(event) => {
            // Arrow-selecting previews the language in place; activating
            // (Enter/Space → click) is what commits AND closes the sheet.
            const next = radioNavTarget(
              LANGS.map((l) => l.code),
              lang,
              event,
              event.currentTarget,
            );
            if (next === null) return;
            setLang(next as Lang);
          }}
        >
          {LANGS.map((l) => (
            <button
              key={l.code}
              lang={l.code}
              role="radio"
              aria-checked={lang === l.code}
              tabIndex={lang === l.code ? 0 : -1}
              onClick={() => {
                setLangSheet(false);
                setLang(l.code);
              }}
            >
              {l.native}
            </button>
          ))}
        </div>
      </Sheet>
    </div>
  );
}
