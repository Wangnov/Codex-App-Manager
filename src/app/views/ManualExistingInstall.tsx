import { Icon } from "../icons";
import { useI18n } from "../i18n";
import { Ring } from "../components";
import { Sheet } from "../Sheet";

export interface ManualExistingCandidate {
  path: string;
  version: string;
  releaseDate?: string | null;
}

export function ManualExistingInstallSheet({
  open,
  candidate,
  error,
  busy,
  labelledBy,
  describedBy,
  onDismiss,
  onPick,
  onAdopt,
}: {
  open: boolean;
  candidate: ManualExistingCandidate | null;
  error: string | null;
  busy: boolean;
  labelledBy: string;
  describedBy: string;
  onDismiss: () => void;
  onPick: () => void;
  onAdopt: () => void;
}) {
  const { t } = useI18n();

  return (
    <Sheet
      open={open}
      onDismiss={onDismiss}
      dismissable={!busy}
      labelledBy={labelledBy}
      describedBy={describedBy}
      initialFocus="first"
    >
      <div className="manual-existing-sheet">
        <Ring icon="folder" />
        <h3 id={labelledBy}>{t("home.manualExisting.title")}</h3>
        <p id={describedBy}>{t("home.manualExisting.body")}</p>

        <div className="manual-existing-picker">
          <div className="sheet-path manual-existing-path">
            {candidate?.path ?? t("home.manualExisting.empty")}
          </div>
          <button className="btn ghost" onClick={onPick} disabled={busy}>
            <Icon name="folder" />
            {t("home.manualExisting.pick")}
          </button>
        </div>

        {candidate ? (
          <div className="list meta manual-existing-meta">
            <div className="row">
              <span className="rtext">
                <span className="rtitle">{t("home.currentVersion")}</span>
              </span>
              <span className="rval version">{candidate.version}</span>
            </div>
            {candidate.releaseDate ? (
              <div className="row">
                <span className="rtext">
                  <span className="rtitle">{t("home.releaseDate")}</span>
                </span>
                <span className="rval">{candidate.releaseDate}</span>
              </div>
            ) : null}
            <div className="row">
              <span className="rtext">
                <span className="rtitle">{t("home.installLocation")}</span>
              </span>
              <span className="rval path" title={candidate.path}>
                {candidate.path}
              </span>
            </div>
          </div>
        ) : null}

        {error ? (
          <div className="manual-existing-error" role="alert" aria-live="assertive">
            <Icon name="alert" />
            <span>{error}</span>
          </div>
        ) : null}

        <div className="row2 sheet-actions">
          <button className="btn ghost" onClick={onDismiss} disabled={busy}>
            {t("confirm.cancel")}
          </button>
          <button className="btn primary" onClick={onAdopt} disabled={!candidate || busy}>
            {t("home.manualExisting.adopt")}
          </button>
        </div>
        <p className="manual-existing-hint">{t("home.manualExisting.hint")}</p>
      </div>
    </Sheet>
  );
}
