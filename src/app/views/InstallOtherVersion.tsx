import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";

import { errorMessage, managerApi } from "../../services/managerApi";
import type {
  HistoricalInstallSelection,
  HistoricalPackageFormat,
  HistoricalRelease,
  HistoricalReleaseArchitecture,
  HistoricalReleaseAsset,
  HistoricalReleaseCatalog,
} from "../../shared/types";
import { Icon } from "../icons";
import { useI18n } from "../i18n";
import { Sheet } from "../Sheet";
import { Toggle } from "../components";

type Platform = "macos" | "windows";
type View = "browse" | "confirm";

interface SelectedPackage {
  release: HistoricalRelease;
  asset: HistoricalReleaseAsset;
  source: "github" | "local";
  localPath: string | null;
  localFileName: string | null;
}

function normalizeArchitecture(value?: string | null): HistoricalReleaseArchitecture | null {
  const normalized = value?.trim().toLowerCase();
  if (normalized === "arm64" || normalized === "aarch64") return "arm64";
  if (normalized === "x64" || normalized === "x86_64" || normalized === "amd64") return "x64";
  return null;
}

function assetPriority(format: HistoricalPackageFormat): number {
  if (format === "dmg") return 0;
  if (format === "zip") return 1;
  return 2;
}

function sortedAssets(assets: HistoricalReleaseAsset[]): HistoricalReleaseAsset[] {
  return [...assets].sort((a, b) => assetPriority(a.format) - assetPriority(b.format));
}

function humanSize(bytes: number): string {
  const mib = bytes / (1024 * 1024);
  return mib >= 1024 ? `${(mib / 1024).toFixed(1)} GB` : `${Math.round(mib)} MB`;
}

function releaseDate(value: string | null): string {
  return value?.slice(0, 10) || "—";
}

function isCurrentRelease(
  release: HistoricalRelease,
  currentVersion: string | null | undefined,
  platform: Platform,
): boolean {
  if (!currentVersion) return false;
  return (
    release.version === currentVersion ||
    (platform === "windows" &&
      release.assets.some((asset) => asset.packageVersion === currentVersion))
  );
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
  allowCurrentVersionInstall = false,
  architecture,
  onDismiss,
  onInstall,
}: {
  open: boolean;
  platform: Platform;
  currentVersion?: string | null;
  allowCurrentVersionInstall?: boolean;
  architecture?: string | null;
  onDismiss: () => void;
  onInstall: (selection: HistoricalInstallSelection, blockUpdates: boolean) => void | Promise<void>;
}) {
  const { t } = useI18n();
  const [view, setView] = useState<View>("browse");
  const [selected, setSelected] = useState<SelectedPackage | null>(null);
  const [blockUpdates, setBlockUpdates] = useState(true);
  const [manualArchitecture, setManualArchitecture] =
    useState<HistoricalReleaseArchitecture | null>(null);
  const [catalog, setCatalog] = useState<HistoricalReleaseCatalog | null>(null);
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const [catalogBusy, setCatalogBusy] = useState(false);
  const [pickBusy, setPickBusy] = useState(false);
  const [submitBusy, setSubmitBusy] = useState(false);
  const titleId = useId();
  const bodyId = useId();
  const toggleTitleId = useId();
  const browseActionRef = useRef<HTMLButtonElement>(null);
  const architectureActionRef = useRef<HTMLButtonElement>(null);
  const backButtonRef = useRef<HTMLButtonElement>(null);
  const catalogRequestRef = useRef(0);
  const hostArchitecture = normalizeArchitecture(architecture);
  // ARM64 hosts can run native ARM64 packages and compatible x64 packages
  // (Rosetta on macOS / x64 emulation on Windows). Keep the detected host as
  // the default, but let the user override it when an older release only ships
  // an x64 asset. An x64 host must remain x64-only.
  const resolvedArchitecture = manualArchitecture ?? hostArchitecture;
  const canChooseArchitecture = hostArchitecture !== "x64";
  const architectureLabel = resolvedArchitecture ?? t("versionPicker.chooseArchitecture");
  const deviceLabel = `${platform === "macos" ? "macOS" : "Windows"} · ${architectureLabel}`;
  const offlineFormats = platform === "macos" ? [".dmg", ".zip"] : [".msix"];
  const canRepairCurrent = platform === "windows" && allowCurrentVersionInstall;

  const releases = useMemo(() => {
    const source = catalog?.releases ?? [];
    const current = source.find((release) => isCurrentRelease(release, currentVersion, platform));
    return current
      ? [current, ...source.filter((release) => release !== current)]
      : source;
  }, [catalog, currentVersion, platform]);
  const recommendedTag = releases.find(
    (release) => !isCurrentRelease(release, currentVersion, platform),
  )?.tag;
  const firstSelectableIndex = releases.findIndex(
    (release) =>
      (!isCurrentRelease(release, currentVersion, platform) || canRepairCurrent) &&
      release.assets.length > 0,
  );

  const loadCatalog = useCallback(
    async (arch: HistoricalReleaseArchitecture) => {
      const request = ++catalogRequestRef.current;
      setCatalogBusy(true);
      setCatalogError(null);
      try {
        const next = await managerApi.historicalReleaseCatalog(platform, arch);
        if (request !== catalogRequestRef.current) return;
        setCatalog(next);
      } catch (cause) {
        if (request !== catalogRequestRef.current) return;
        setCatalog(null);
        setCatalogError(errorMessage(cause));
      } finally {
        if (request === catalogRequestRef.current) setCatalogBusy(false);
      }
    },
    [platform],
  );

  useEffect(() => {
    if (!open) return;
    setView("browse");
    setSelected(null);
    setBlockUpdates(true);
    setManualArchitecture(null);
    setCatalog(null);
    setCatalogError(null);
    setPickBusy(false);
    setSubmitBusy(false);
  }, [allowCurrentVersionInstall, architecture, currentVersion, open, platform]);

  useEffect(() => {
    if (!open || !resolvedArchitecture) return;
    void loadCatalog(resolvedArchitecture);
    return () => {
      // Invalidate the automatic request and any manual retry tied to the
      // architecture/view being left. A late ARM64 response must never replace
      // the x64 catalog selected afterwards (or repopulate a reopened sheet).
      catalogRequestRef.current += 1;
    };
  }, [loadCatalog, open, resolvedArchitecture]);

  useEffect(() => {
    if (!open) return;
    if (view === "browse") {
      (resolvedArchitecture ? browseActionRef : architectureActionRef).current?.focus();
    } else {
      backButtonRef.current?.focus();
    }
  }, [catalogBusy, open, resolvedArchitecture, view]);

  const chooseRelease = (release: HistoricalRelease) => {
    if (isCurrentRelease(release, currentVersion, platform) && !canRepairCurrent) return;
    const asset = sortedAssets(release.assets)[0];
    if (!asset) return;
    setSelected({
      release,
      asset,
      source: "github",
      localPath: null,
      localFileName: null,
    });
    setCatalogError(null);
    setView("confirm");
  };

  const chooseOffline = async () => {
    if (!resolvedArchitecture || pickBusy) return;
    setPickBusy(true);
    setCatalogError(null);
    try {
      const local = await managerApi.historicalPickLocalPackage(platform, resolvedArchitecture);
      if (!local) return;
      setSelected({
        release: {
          tag: local.releaseTag,
          version: local.version,
          publishedAt: null,
          assets: [
            {
              name: local.assetName,
              size: local.size,
              architecture: local.architecture,
              format: local.format,
              packageVersion: local.packageVersion,
            },
          ],
        },
        asset: {
          name: local.assetName,
          size: local.size,
          architecture: local.architecture,
          format: local.format,
          packageVersion: local.packageVersion,
        },
        source: "local",
        localPath: local.path,
        localFileName: local.fileName,
      });
      setView("confirm");
    } catch (cause) {
      setCatalogError(errorMessage(cause));
    } finally {
      setPickBusy(false);
    }
  };

  const submit = async () => {
    if (!selected || submitBusy) return;
    setSubmitBusy(true);
    setCatalogError(null);
    try {
      await onInstall(
        {
          releaseTag: selected.release.tag,
          version: selected.release.version,
          assetName: selected.asset.name,
          architecture: selected.asset.architecture,
          format: selected.asset.format,
          packageVersion: selected.asset.packageVersion,
          localPath: selected.localPath,
          localFileName: selected.localFileName,
        },
        blockUpdates,
      );
    } catch (cause) {
      setCatalogError(errorMessage(cause));
      setSubmitBusy(false);
    }
  };

  const packageLabel = selected
    ? `${selected.asset.format.toUpperCase()} · ${selected.asset.architecture}${
        selected.asset.packageVersion ? ` · ${selected.asset.packageVersion}` : ""
      }`
    : "";
  const repairingCurrent = Boolean(
    canRepairCurrent &&
      selected &&
      isCurrentRelease(selected.release, currentVersion, platform),
  );

  return (
    <Sheet
      open={open}
      onDismiss={onDismiss}
      dismissable={!pickBusy && !submitBusy}
      labelledBy={titleId}
      describedBy={bodyId}
      initialFocus="first"
      centeredInExpanded
    >
      <div className={`version-picker-sheet view-${view}`}>
        {view === "browse" ? (
          <>
            <div className="version-picker-heading">
              <span className="version-picker-kicker">GitHub Releases</span>
              <h3 id={titleId}>{t("versionPicker.title")}</h3>
              <p id={bodyId}>{t("versionPicker.body")}</p>
            </div>

            <div className="version-device">
              <span>
                <Icon name="shield" />
                {deviceLabel}
              </span>
              {canChooseArchitecture ? (
                <div
                  className="version-architecture-choice"
                  role="group"
                  aria-label={t("versionPicker.chooseArchitecture")}
                >
                  <button
                    ref={architectureActionRef}
                    type="button"
                    onClick={() => setManualArchitecture("arm64")}
                    aria-pressed={resolvedArchitecture === "arm64"}
                  >
                    arm64
                  </button>
                  <button
                    type="button"
                    onClick={() => setManualArchitecture("x64")}
                    aria-pressed={resolvedArchitecture === "x64"}
                  >
                    x64
                  </button>
                </div>
              ) : null}
            </div>

            <div className="version-source">
              <span>
                <Icon name="download" />
                Wangnov/codex-app-mirror
              </span>
              <small>
                {resolvedArchitecture
                  ? t("versionPicker.sourceHint")
                  : t("versionPicker.chooseArchitecture")}
              </small>
            </div>

            {catalogError ? (
              <div className="version-picker-error" role="alert">
                <span>{catalogError}</span>
                {resolvedArchitecture ? (
                  <button type="button" onClick={() => loadCatalog(resolvedArchitecture)}>
                    {t("home.recheck")}
                  </button>
                ) : null}
              </div>
            ) : null}

            <div className="version-list" aria-busy={catalogBusy || pickBusy}>
              {catalogBusy ? (
                <div className="version-picker-loading">
                  <Icon name="loader" />
                  {t("progress.preparing")}
                </div>
              ) : null}

              {!catalogBusy
                ? releases.map((release, index) => {
                    const current = isCurrentRelease(release, currentVersion, platform);
                    const repairableCurrent = current && canRepairCurrent;
                    const recommended = release.tag === recommendedTag;
                    const assets = sortedAssets(release.assets);
                    const size = assets[0]?.size ?? 0;
                    return (
                      <button
                        ref={index === firstSelectableIndex ? browseActionRef : undefined}
                        key={release.tag}
                        className={`version-option${recommended ? " recommended" : ""}${
                          current ? " current" : ""
                        }`}
                        onClick={() => chooseRelease(release)}
                        disabled={(current && !repairableCurrent) || !resolvedArchitecture}
                      >
                        <span className="version-rail" aria-hidden="true">
                          <span />
                        </span>
                        <span className="version-option-copy">
                          <span className="version-option-topline">
                            <span className="version-number">{release.version}</span>
                            {recommended ? (
                              <span className="version-badge recommended">
                                {t("versionPicker.recommended")}
                              </span>
                            ) : null}
                            {current ? (
                              <span className="version-badge current">
                                {t("versionPicker.current")}
                              </span>
                            ) : null}
                            {repairableCurrent ? (
                              <span className="version-badge recommended">
                                {t("versionPicker.repair")}
                              </span>
                            ) : null}
                          </span>
                          <span className="version-option-meta">
                            GitHub Releases <span aria-hidden="true">·</span>{" "}
                            {releaseDate(release.publishedAt)} <span aria-hidden="true">·</span>{" "}
                            {humanSize(size)}
                          </span>
                          {!current || repairableCurrent ? (
                            <span className="version-compatible">
                              <Icon name="check" />
                              {t(
                                repairableCurrent
                                  ? "versionPicker.repair"
                                  : "versionPicker.compatible",
                              )}
                            </span>
                          ) : null}
                        </span>
                        {!current || repairableCurrent ? (
                          <Icon name="chevron" className="version-chevron" />
                        ) : null}
                      </button>
                    );
                  })
                : null}

              {!catalogBusy && resolvedArchitecture && releases.length === 0 ? (
                <div className="version-picker-loading">{t("versionPicker.body")}</div>
              ) : null}

              <button
                className="version-offline"
                onClick={() => void chooseOffline()}
                disabled={!resolvedArchitecture || pickBusy || submitBusy}
              >
                <span className="version-offline-icon">
                  <Icon name={pickBusy ? "loader" : "folder"} />
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
            <button
              ref={backButtonRef}
              className="version-picker-back"
              onClick={() => {
                setSelected(null);
                setCatalogError(null);
                setView("browse");
              }}
              disabled={submitBusy}
            >
              <Icon name="back" />
              {t("nav.back")}
            </button>
            <div className="version-picker-heading confirm-heading">
              <span className="version-picker-kicker">GitHub Releases</span>
              <h3 id={titleId}>
                {t(
                  repairingCurrent
                    ? "versionPicker.confirmRepairTitle"
                    : "versionPicker.confirmTitle",
                  { version: selected.release.version },
                )}
              </h3>
              <p id={bodyId}>
                {repairingCurrent
                  ? t("versionPicker.confirmRepairBody")
                  : currentVersion
                  ? t("versionPicker.confirmBody", {
                      current: currentVersion,
                      target: selected.release.version,
                    })
                  : selected.source === "local"
                    ? t("versionPicker.confirmLocalFreshBody", {
                        target: selected.release.version,
                      })
                    : t("versionPicker.confirmFreshBody", { target: selected.release.version })}
              </p>
            </div>

            <div
              className={`version-transition${
                currentVersion && !repairingCurrent ? "" : " single"
              }`}
            >
              {currentVersion && !repairingCurrent ? (
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
                <strong>{selected.release.version}</strong>
              </span>
            </div>

            {selected.source === "github" && selected.release.assets.length > 1 ? (
              <div className="version-package-choice" role="group" aria-label={packageLabel}>
                {sortedAssets(selected.release.assets).map((asset) => (
                  <button
                    type="button"
                    key={asset.name}
                    className={asset.name === selected.asset.name ? "selected" : ""}
                    onClick={() => setSelected({ ...selected, asset })}
                    disabled={submitBusy}
                  >
                    {asset.format.toUpperCase()}
                    <small>{humanSize(asset.size)}</small>
                  </button>
                ))}
              </div>
            ) : null}

            <div className={`version-local-file${selected.source === "github" ? " github-asset" : ""}`}>
              <Icon name={selected.source === "github" ? "download" : "folder"} />
              <span>
                <small>
                  {selected.source === "github"
                    ? t("versionPicker.githubAsset")
                    : t("versionPicker.localPackage")}
                </small>
                <strong title={selected.localFileName ?? selected.asset.name}>
                  {selected.localFileName ?? selected.asset.name}
                </strong>
              </span>
            </div>

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

            {catalogError ? (
              <div className="version-picker-error" role="alert">
                {catalogError}
              </div>
            ) : null}

            <div className="row2 sheet-actions version-picker-actions">
              <button className="btn ghost" onClick={onDismiss} disabled={submitBusy}>
                {t("confirm.cancel")}
              </button>
              <button className="btn primary" onClick={() => void submit()} disabled={submitBusy}>
                <Icon name={submitBusy ? "loader" : "download"} />
                {submitBusy
                  ? t("progress.preparing")
                  : repairingCurrent && selected.source === "github"
                    ? t("versionPicker.repairInstall")
                  : selected.source === "local"
                    ? t("versionPicker.verifyInstall")
                    : t("versionPicker.downloadInstall")}
              </button>
            </div>
          </>
        ) : null}
      </div>
    </Sheet>
  );
}
