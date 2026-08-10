import { useEffect, useId, useMemo, useRef, useState } from "react";

import { Icon } from "../icons";
import { useI18n } from "../i18n";
import { Sheet } from "../Sheet";
import { Ring, Toggle } from "../components";

type Platform = "macos" | "windows";
type View = "browse" | "confirm" | "done";
type PackageArchitecture = "arm64" | "x64";

interface MockVersion {
  /** Human-facing Codex app version, shared by macOS and Windows. */
  version: string;
  released: string;
  size: string;
  source: "installed" | "github" | "local";
  recommended?: boolean;
  current?: boolean;
  fileName?: string;
  windowsPackage?: {
    packageVersion: string;
    packageMonikers: Record<PackageArchitecture, string>;
  };
}

function windowsRelease(
  version: string,
  packageVersion: string,
  released: string,
  size: string,
  recommended = false,
): MockVersion {
  const moniker = (architecture: PackageArchitecture) =>
    `OpenAI.Codex_${packageVersion}_${architecture}__2p2nqsd0c76g0`;
  return {
    version,
    released,
    size,
    source: "github",
    recommended,
    windowsPackage: {
      packageVersion,
      packageMonikers: {
        arm64: moniker("arm64"),
        x64: moniker("x64"),
      },
    },
  };
}

function mockVersions(platform: Platform, currentVersion: string): MockVersion[] {
  const current: MockVersion = {
    version: currentVersion,
    released: "2026-08-07",
    size: platform === "macos" ? "526 MB" : "667 MB",
    source: "installed",
    current: true,
  };
  const history =
    platform === "macos"
      ? [
          {
            version: "26.727.51351",
            released: "2026-07-31",
            size: "534 MB",
            source: "github" as const,
            recommended: true,
          },
          {
            version: "26.721.81911",
            released: "2026-07-25",
            size: "529 MB",
            source: "github" as const,
          },
          {
            version: "26.721.41059",
            released: "2026-07-21",
            size: "521 MB",
            source: "github" as const,
          },
        ]
      : [
          windowsRelease("26.727.51351", "26.727.6591.0", "2026-07-31", "724 MB", true),
          windowsRelease("26.721.81911", "26.721.11231.0", "2026-07-25", "718 MB"),
          windowsRelease("26.721.41059", "26.721.4979.0", "2026-07-21", "710 MB"),
        ];
  return currentVersion.trim()
    ? [
        current,
        ...history.filter(
          (item) =>
            item.version !== currentVersion &&
            item.windowsPackage?.packageVersion !== currentVersion,
        ),
      ]
    : history;
}

function normalizeArchitecture(value?: string | null): PackageArchitecture | null {
  const normalized = value?.trim().toLowerCase();
  if (normalized === "arm64" || normalized === "aarch64") return "arm64";
  if (normalized === "x64" || normalized === "x86_64" || normalized === "amd64") return "x64";
  return null;
}

export function InstallOtherVersionEntry({
  disabled,
  onOpen,
}: {
  disabled: boolean;
  onOpen: () => void;
}) {
  const { t } = useI18n();
  return (
    <div className="other-version-entry">
      <button className="linkbtn subtle" onClick={onOpen} disabled={disabled}>
        <Icon name="list" />
        {t("versionPicker.entry")}
      </button>
    </div>
  );
}

export function InstallOtherVersionSheet({
  open,
  platform,
  currentVersion,
  architecture,
  onDismiss,
}: {
  open: boolean;
  platform: Platform;
  currentVersion?: string | null;
  architecture?: string | null;
  onDismiss: () => void;
}) {
  const { t } = useI18n();
  const [view, setView] = useState<View>("browse");
  const [selected, setSelected] = useState<MockVersion | null>(null);
  const [blockUpdates, setBlockUpdates] = useState(true);
  const titleId = useId();
  const bodyId = useId();
  const toggleTitleId = useId();
  const browseActionRef = useRef<HTMLButtonElement>(null);
  const architectureActionRef = useRef<HTMLButtonElement>(null);
  const backButtonRef = useRef<HTMLButtonElement>(null);
  const doneButtonRef = useRef<HTMLButtonElement>(null);
  const [manualArchitecture, setManualArchitecture] = useState<PackageArchitecture | null>(null);
  const versions = useMemo(
    () => mockVersions(platform, currentVersion ?? ""),
    [currentVersion, platform],
  );
  const resolvedArchitecture = normalizeArchitecture(architecture) ?? manualArchitecture;
  const architectureLabel = resolvedArchitecture ?? t("versionPicker.chooseArchitecture");
  const deviceLabel = `${platform === "macos" ? "macOS" : "Windows"} · ${architectureLabel}`;
  const packageLabel =
    platform === "macos"
      ? `${architectureLabel} · DMG / ZIP`
      : `MSIX · ${architectureLabel}`;
  const confirmationPackageLabel = selected?.windowsPackage
    ? `${packageLabel} · ${selected.windowsPackage.packageVersion}`
    : packageLabel;
  const offlineFormats = platform === "macos" ? [".dmg", ".zip"] : [".msix"];

  useEffect(() => {
    if (!open) return;
    setView("browse");
    setSelected(null);
    setBlockUpdates(true);
    setManualArchitecture(null);
  }, [architecture, currentVersion, open, platform]);

  useEffect(() => {
    if (!open) return;
    if (view === "browse") {
      (resolvedArchitecture ? browseActionRef : architectureActionRef).current?.focus();
    }
    if (view === "confirm") backButtonRef.current?.focus();
    if (view === "done") doneButtonRef.current?.focus();
  }, [open, resolvedArchitecture, view]);

  const choose = (candidate: MockVersion) => {
    if (candidate.current) return;
    setSelected(candidate);
    setView("confirm");
  };

  const chooseOffline = () => {
    if (!resolvedArchitecture) return;
    const candidate =
      versions.find((version) => version.recommended && !version.current) ??
      versions.find((version) => !version.current);
    if (!candidate) return;
    const packageMoniker = candidate.windowsPackage?.packageMonikers[resolvedArchitecture];
    setSelected({
      ...candidate,
      source: "local",
      fileName:
        platform === "macos"
          ? `Codex-mac-${resolvedArchitecture}.dmg`
          : packageMoniker
            ? `${packageMoniker}.Msix`
            : undefined,
    });
    setView("confirm");
  };

  const goBack = () => {
    setSelected(null);
    setView("browse");
  };

  return (
    <Sheet
      open={open}
      onDismiss={onDismiss}
      labelledBy={titleId}
      describedBy={bodyId}
      initialFocus="first"
      centeredInExpanded
    >
      <div className={`version-picker-sheet view-${view}`}>
        {view === "browse" ? (
          <>
            <div className="version-picker-heading">
              <span className="version-picker-kicker">{t("versionPicker.preview")}</span>
              <h3 id={titleId}>{t("versionPicker.title")}</h3>
              <p id={bodyId}>{t("versionPicker.body")}</p>
            </div>

            <div className="version-device">
              <span>
                <Icon name="shield" />
                {deviceLabel}
              </span>
              {resolvedArchitecture ? (
                platform === "macos" ? (
                  t("versionPicker.filtered", { count: 2 })
                ) : null
              ) : (
                <div
                  className="version-architecture-choice"
                  role="group"
                  aria-label={t("versionPicker.chooseArchitecture")}
                >
                  <button
                    ref={architectureActionRef}
                    type="button"
                    onClick={() => setManualArchitecture("arm64")}
                  >
                    arm64
                  </button>
                  <button type="button" onClick={() => setManualArchitecture("x64")}>
                    x64
                  </button>
                </div>
              )}
            </div>

            <div className="version-source">
              <span>
                <Icon name="download" />
                GitHub Releases
              </span>
              <small>
                {resolvedArchitecture
                  ? t("versionPicker.sourceHint")
                  : t("versionPicker.chooseArchitecture")}
              </small>
            </div>

            <div className="version-list">
              {versions.map((candidate, index) => (
                <button
                  ref={
                    index === versions.findIndex((version) => !version.current)
                      ? browseActionRef
                      : undefined
                  }
                  key={`${candidate.version}-${candidate.current ? "current" : "history"}`}
                  className={`version-option${candidate.recommended ? " recommended" : ""}${
                    candidate.current ? " current" : ""
                  }`}
                  onClick={() => choose(candidate)}
                  disabled={candidate.current || !resolvedArchitecture}
                >
                  <span className="version-rail" aria-hidden="true">
                    <span />
                  </span>
                  <span className="version-option-copy">
                    <span className="version-option-topline">
                      <span className="version-number">{candidate.version}</span>
                      {candidate.recommended ? (
                        <span className="version-badge recommended">
                          {t("versionPicker.recommended")}
                        </span>
                      ) : null}
                      {candidate.current ? (
                        <span className="version-badge current">{t("versionPicker.current")}</span>
                      ) : null}
                    </span>
                    <span className="version-option-meta">
                      {candidate.source === "github" ? (
                        <>
                          GitHub Releases <span aria-hidden="true">·</span>{" "}
                        </>
                      ) : null}
                      {candidate.released} <span aria-hidden="true">·</span> {candidate.size}
                    </span>
                    {!candidate.current ? (
                      <span className="version-compatible">
                        <Icon name="check" />
                        {t("versionPicker.compatible")}
                      </span>
                    ) : null}
                  </span>
                  {!candidate.current ? <Icon name="chevron" className="version-chevron" /> : null}
                </button>
              ))}

              <button
                className="version-offline"
                onClick={chooseOffline}
                disabled={!resolvedArchitecture}
              >
                <span className="version-offline-icon">
                  <Icon name="folder" />
                </span>
                <span className="version-option-copy">
                  <span className="version-offline-title">{t("versionPicker.offline")}</span>
                  <span className="version-option-meta">
                    {t("versionPicker.offlineBody")}
                    <span className="version-format-list">
                      {offlineFormats.map((format) => (
                        <span key={format}>{format}</span>
                      ))}
                    </span>
                  </span>
                </span>
                <Icon name="chevron" className="version-chevron" />
              </button>
            </div>
          </>
        ) : null}

        {view === "confirm" && selected ? (
          <>
            <button ref={backButtonRef} className="version-picker-back" onClick={goBack}>
              <Icon name="back" />
              {t("nav.back")}
            </button>
            <div className="version-picker-heading confirm-heading">
              <span className="version-picker-kicker">{t("versionPicker.preview")}</span>
              <h3 id={titleId}>{t("versionPicker.confirmTitle", { version: selected.version })}</h3>
              <p id={bodyId}>
                {currentVersion
                  ? t("versionPicker.confirmBody", {
                      current: currentVersion,
                      target: selected.version,
                    })
                  : selected.source === "local"
                    ? t("versionPicker.confirmLocalFreshBody", { target: selected.version })
                    : t("versionPicker.confirmFreshBody", { target: selected.version })}
              </p>
            </div>

            <div
              className={`version-transition${currentVersion ? "" : " single"}`}
            >
              {currentVersion ? (
                <>
                  <span>
                    <small>{t("versionPicker.current")}</small>
                    <strong>{currentVersion}</strong>
                  </span>
                  <Icon name="chevron" />
                </>
              ) : null}
              <span className="target">
                <small>{t("versionPicker.target")}</small>
                <strong>{selected.version}</strong>
              </span>
            </div>

            {selected.source === "github" ? (
              <div className="version-local-file github-asset">
                <Icon name="download" />
                <span>
                  <small>{t("versionPicker.githubAsset")}</small>
                  <strong>GitHub Releases · {confirmationPackageLabel}</strong>
                </span>
              </div>
            ) : null}

            {selected.source === "local" && selected.fileName ? (
              <div className="version-local-file">
                <Icon name="folder" />
                <span>
                  <small>{t("versionPicker.localPackage")}</small>
                  <strong title={selected.fileName}>{selected.fileName}</strong>
                </span>
              </div>
            ) : null}

            <div className="version-checks">
              <div>
                <Icon name="shield" />
                <span>{t("home.official")}</span>
                <Icon name="check" />
              </div>
              <div>
                <Icon name="sliders" />
                <span>{packageLabel}</span>
                <Icon name="check" />
              </div>
              <div>
                <Icon name="check" />
                <span>{t("versionPicker.compatible")}</span>
                <Icon name="check" />
              </div>
            </div>

            <div className="version-update-lock">
              <span className="rtext">
                <span className="rtitle" id={toggleTitleId}>
                  {t("settings.general.disableCodexSelfUpdates")}
                </span>
                <span className="rsub">{t("versionPicker.blockUpdatesNote")}</span>
              </span>
              <Toggle
                checked={blockUpdates}
                onChange={setBlockUpdates}
                ariaLabelledBy={toggleTitleId}
              />
            </div>

            <div className="row2 sheet-actions version-picker-actions">
              <button className="btn ghost" onClick={onDismiss}>
                {t("confirm.cancel")}
              </button>
              <button className="btn primary" onClick={() => setView("done")}>
                <Icon name="download" />
                {selected.source === "local"
                  ? t("versionPicker.verifyInstall")
                  : t("versionPicker.downloadInstall")}
              </button>
            </div>
          </>
        ) : null}

        {view === "done" ? (
          <div className="version-picker-done">
            <Ring icon="check" variant="success" />
            <h3 id={titleId}>{t("versionPicker.doneTitle")}</h3>
            <p id={bodyId}>{t("versionPicker.doneBody")}</p>
            <button ref={doneButtonRef} className="btn primary" onClick={onDismiss}>
              {t("success.done")}
            </button>
          </div>
        ) : null}
      </div>
    </Sheet>
  );
}
