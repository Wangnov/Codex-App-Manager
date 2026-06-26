import { useEffect, useId, useState } from "react";

import { errorMessage, managerApi } from "../../services/managerApi";
import type { AppSettings, ProxyMode, UpdateSourceKind, WindowsInstallMode } from "../../shared/types";
import { DEFAULT_SETTINGS } from "../../shared/types";
import { Icon } from "../icons";
import { useI18n, LANGS, type TFn, type TKey } from "../i18n";
import { useTheme, type ThemeMode } from "../theme";
import { NavBar, Segmented, Toggle } from "../components";
import { isWindows } from "../platform";
import { Sheet } from "../Sheet";
import { useSettingsSaver } from "./useSettingsSaver";

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

function samePath(a: string, b: string): boolean {
  const norm = (value: string) =>
    value.trim().replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();
  return norm(a) === norm(b);
}

export function Settings({
  onBack,
  onOpenAbout,
  onOpenUninstall,
  onOpenConfig,
}: {
  onBack: () => void;
  onOpenAbout: () => void;
  onOpenUninstall: () => void;
  onOpenConfig: () => void;
}) {
  const { t, lang, setLang } = useI18n();
  const { mode, setMode } = useTheme();
  const win = isWindows();
  const {
    settings: s,
    status: saveStatus,
    error: saveError,
    update,
    retry,
    reset,
    setDraft,
  } = useSettingsSaver(DEFAULT_SETTINGS);
  const [defaultInstallRoot, setDefaultInstallRoot] = useState(DEFAULT_SETTINGS.installRoot);
  const [autostart, setAutostart] = useState(false);
  const [commandError, setCommandError] = useState<string | null>(null);
  const [langSheet, setLangSheet] = useState(false);
  const [customIntervalOpen, setCustomIntervalOpen] = useState(false);
  const langTitleId = useId();

  useEffect(() => {
    void managerApi.getSettings().then(reset).catch(() => undefined);
    void managerApi.getAutostart().then(setAutostart).catch(() => undefined);
    if (win) {
      void managerApi.winDefaultInstallRoot().then(setDefaultInstallRoot).catch(() => undefined);
    }
  }, [reset, win]);

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
  const error = commandError ?? saveError;
  const customInterval = customIntervalOpen || !FREQUENCY_PRESETS.some(
    (option) => option.seconds === s.periodicCheckIntervalSeconds,
  );
  const intervalParts = splitInterval(s.periodicCheckIntervalSeconds);
  const intervalLabel = formatInterval(s.periodicCheckIntervalSeconds, t);

  const updateIntervalPart = (
    part: "hours" | "minutes" | "seconds",
    value: string,
  ) => {
    const nextParts = { ...splitInterval(s.periodicCheckIntervalSeconds) };
    nextParts[part] = clampPart(value, part === "hours" ? MAX_CUSTOM_HOURS : 59);
    setCommandError(null);
    update({ ...s, periodicCheckIntervalSeconds: intervalFromParts(nextParts) });
  };

  return (
    <div className="pop">
      <NavBar title={t("settings.title")} onBack={onBack} />
      <div className="scroll view" inert={langSheet ? true : undefined}>
        {saveStatus === "saving" ? (
          <div className="banner info" role="status">
            <Icon name="loader" />
            <span>{t("settings.saving")}</span>
          </div>
        ) : null}
        {error ? (
          <div className="banner err">
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

        {/* 更新源 */}
        <div className="group">
          <div className="group-h">{t("settings.source.header")}</div>
          <div className="list">
            {availableSources.map((src) => (
              <button
                key={src.kind}
                className="row"
                aria-checked={s.source === src.kind}
                onClick={() => {
                  setCommandError(null);
                  update({ ...s, source: src.kind });
                }}
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
                  value={s.customUrl}
                  placeholder={t("settings.source.customPlaceholder")}
                  onChange={(e) => setDraft({ ...s, customUrl: e.target.value })}
                  onBlur={() => {
                    setCommandError(null);
                    update(s);
                  }}
                />
              </div>
            ) : null}
          </div>
        </div>

        {win ? (
          <div className="group">
            <div className="group-h">{t("settings.windows.header")}</div>
            <div className="list">
              {WINDOWS_INSTALL_MODES.map((mode) => (
                <button
                  key={mode.kind}
                  className="row"
                  aria-checked={s.windowsInstallMode === mode.kind}
                  onClick={() => {
                    setCommandError(null);
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

        {/* 通用 */}
        <div className="group">
          <div className="group-h">{t("settings.general.header")}</div>
          <div className="list">
            <div className="row">
              <span className="rtext">
                <span className="rtitle">{t("settings.general.checkOnStartup")}</span>
              </span>
              <Toggle
                checked={s.checkOnStartup}
                onChange={(v) => {
                  setCommandError(null);
                  update({ ...s, checkOnStartup: v });
                }}
              />
            </div>
            <div className="row">
              <span className="rtext">
                <span className="rtitle">{t("settings.general.periodicCheck")}</span>
              </span>
              <Toggle
                checked={s.periodicCheck}
                onChange={(v) => {
                  setCommandError(null);
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
                <span className="rtitle">{t("settings.general.askBefore")}</span>
              </span>
              <Toggle
                checked={s.askBefore}
                onChange={(v) => {
                  setCommandError(null);
                  update({ ...s, askBefore: v });
                }}
              />
            </div>
            <div className="row">
              <span className="rtext">
                <span className="rtitle">{t("settings.general.disableCodexSelfUpdates")}</span>
                <span className="rsub">{t("settings.general.disableCodexSelfUpdatesNote")}</span>
              </span>
              <Toggle
                checked={s.disableCodexSelfUpdates}
                onChange={(v) => {
                  setCommandError(null);
                  update({ ...s, disableCodexSelfUpdates: v });
                }}
              />
            </div>
            <div className="row">
              <span className="rtext">
                <span className="rtitle">{t("settings.general.autostart")}</span>
                <span className="rsub">{t("settings.general.autostartNote")}</span>
              </span>
              <Toggle checked={autostart} onChange={toggleAutostart} />
            </div>
            <div className="row">
              <span className="rtext">
                <span className="rtitle">{t("settings.general.confirmClose")}</span>
              </span>
              <Toggle
                checked={s.confirmClose}
                onChange={(v) => {
                  setCommandError(null);
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
                onChange={(key) => {
                  setCommandError(null);
                  const next = { ...s, proxyMode: key as ProxyMode };
                  if (key === "custom" && !s.customProxyUrl.trim()) {
                    setDraft(next);
                    return;
                  }
                  update(next);
                }}
              />
              {s.proxyMode === "custom" ? (
                <div style={{ marginTop: 10 }}>
                  <input
                    className="input mono"
                    value={s.customProxyUrl}
                    placeholder={t("settings.network.proxyPlaceholder")}
                    onChange={(e) => setDraft({ ...s, customProxyUrl: e.target.value })}
                    onBlur={(e) => {
                      setCommandError(null);
                      update({ ...s, customProxyUrl: e.currentTarget.value });
                    }}
                  />
                </div>
              ) : null}
            </div>
          </div>
        </div>

        {/* 更多 */}
        <div className="group">
          <div className="group-h">{t("settings.more.header")}</div>
          <div className="list">
            <button className="row" onClick={onOpenConfig}>
              <Icon name="sliders" className="ricon" />
              <span className="rtext">
                <span className="rtitle">{t("settings.more.config")}</span>
              </span>
              <span className="tag soon">{t("settings.more.soon")}</span>
              <Icon name="chevron" className="chev" />
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
        open={langSheet}
        onDismiss={() => setLangSheet(false)}
        labelledBy={langTitleId}
        initialFocus="first"
      >
        <h3 id={langTitleId}>{t("settings.appearance.language")}</h3>
        <div className="langgrid" style={{ marginTop: 14 }}>
          {LANGS.map((l) => (
            <button
              key={l.code}
              lang={l.code}
              aria-selected={lang === l.code}
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
