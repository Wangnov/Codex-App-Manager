import { useEffect, useState } from "react";

import { errorMessage, managerApi } from "../../services/managerApi";
import type { AppSettings, UpdateSourceKind, WindowsInstallMode } from "../../shared/types";
import { DEFAULT_SETTINGS } from "../../shared/types";
import { Icon } from "../icons";
import { useI18n, LANGS, type TKey } from "../i18n";
import { useTheme, type ThemeMode } from "../theme";
import { NavBar, Toggle } from "../components";
import { isWindows } from "../platform";

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
  const [s, setS] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [defaultInstallRoot, setDefaultInstallRoot] = useState(DEFAULT_SETTINGS.installRoot);
  const [autostart, setAutostart] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void managerApi.getSettings().then(setS).catch(() => undefined);
    void managerApi.getAutostart().then(setAutostart).catch(() => undefined);
    if (win) {
      void managerApi.winDefaultInstallRoot().then(setDefaultInstallRoot).catch(() => undefined);
    }
  }, [win]);

  const save = (next: AppSettings) => {
    setError(null);
    setS(next);
    void managerApi
      .setSettings(next)
      .then(setS)
      .catch((cause) => setError(errorMessage(cause)));
  };

  const pickInstallRoot = async () => {
    setError(null);
    try {
      const path = await managerApi.winPickInstallDir();
      if (!path) return;
      setS(await managerApi.winSetInstallRoot(path));
    } catch (cause) {
      setError(errorMessage(cause));
    }
  };

  const resetInstallRoot = async () => {
    setError(null);
    try {
      setS(await managerApi.winResetInstallRoot());
    } catch (cause) {
      setError(errorMessage(cause));
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

  return (
    <div className="pop">
      <NavBar title={t("settings.title")} onBack={onBack} />
      <div className="scroll view">
        {error ? (
          <div className="banner err">
            <Icon name="alert" />
            <span>{error}</span>
          </div>
        ) : null}

        {/* 更新源 */}
        <div className="group">
          <div className="group-h">{t("settings.source.header")}</div>
          <div className="list">
            {SOURCES.map((src) => (
              <button
                key={src.kind}
                className="row"
                aria-checked={s.source === src.kind}
                onClick={() => save({ ...s, source: src.kind })}
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
                  onChange={(e) => setS({ ...s, customUrl: e.target.value })}
                  onBlur={() => save(s)}
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
                  onClick={() => save({ ...s, windowsInstallMode: mode.kind })}
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
                <span className="rtitle">{t("settings.general.autoCheck")}</span>
              </span>
              <Toggle checked={s.autoCheck} onChange={(v) => save({ ...s, autoCheck: v })} />
            </div>
            <div className="row">
              <span className="rtext">
                <span className="rtitle">{t("settings.general.askBefore")}</span>
              </span>
              <Toggle checked={s.askBefore} onChange={(v) => save({ ...s, askBefore: v })} />
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
                onChange={(v) => save({ ...s, confirmClose: v })}
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
              <div className="seg">
                {themes.map((th) => (
                  <button
                    key={th.v}
                    aria-selected={mode === th.v}
                    onClick={() => setMode(th.v)}
                  >
                    {t(th.k)}
                  </button>
                ))}
              </div>
            </div>
            <div className="row" style={{ display: "block" }}>
              <div className="rtitle" style={{ marginBottom: 8 }}>
                {t("settings.appearance.language")}
              </div>
              <div className="langgrid">
                {LANGS.map((l) => (
                  <button
                    key={l.code}
                    lang={l.code}
                    aria-selected={lang === l.code}
                    onClick={() => setLang(l.code)}
                  >
                    {l.native}
                  </button>
                ))}
              </div>
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
    </div>
  );
}
