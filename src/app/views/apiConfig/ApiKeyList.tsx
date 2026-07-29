import { useEffect, useId, useMemo, useState } from "react";

import { errorCode, managerApi } from "../../../services/managerApi";
import type {
  ApiConfigKey,
  ApiConfigKeyList as ApiConfigKeyListModel,
  ApiConfigSession,
  ApiConfigWriteReport,
} from "../../../shared/types";
import { Icon } from "../../icons";
import { useI18n, type TKey } from "../../i18n";
import { Sheet } from "../../Sheet";
import { apiConfigErrorText, apiConfigWarningText } from "./errors";

const PAGE_SIZE = 20;

const STATUS_KEYS = {
  active: "config.status.active",
  inactive: "config.status.inactive",
  quota_exhausted: "config.status.quotaExhausted",
  expired: "config.status.expired",
} as const satisfies Record<ApiConfigKey["status"], TKey>;

function statusKey(status: ApiConfigKey["status"]): TKey {
  return STATUS_KEYS[status];
}

function formatExpiry(expiresAt: string | null, lang: string, noExpiry: string): string {
  if (!expiresAt) return noExpiry;
  const date = new Date(expiresAt);
  if (!Number.isFinite(date.getTime())) return expiresAt;
  return new Intl.DateTimeFormat(lang, { dateStyle: "medium" }).format(date);
}

export function ApiKeyList({
  session,
  keys,
  initialErrorCode,
  onKeysChange,
  onSessionChange,
  onLogout,
}: {
  session: ApiConfigSession;
  keys: ApiConfigKeyListModel;
  initialErrorCode: string | null;
  onKeysChange: (keys: ApiConfigKeyListModel) => void;
  onSessionChange: (session: ApiConfigSession) => void;
  onLogout: (session: ApiConfigSession) => void;
}) {
  const { t, lang } = useI18n();
  const [page, setPage] = useState(1);
  const [refreshing, setRefreshing] = useState(false);
  const [importingId, setImportingId] = useState<number | null>(null);
  const [writingId, setWritingId] = useState<number | null>(null);
  const [confirmKey, setConfirmKey] = useState<ApiConfigKey | null>(null);
  const [loggingOut, setLoggingOut] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [restartFailed, setRestartFailed] = useState(false);
  const [actionError, setActionError] = useState<string | null>(initialErrorCode);
  const [ccsNotice, setCcsNotice] = useState<number | null>(null);
  const [writeReport, setWriteReport] = useState<ApiConfigWriteReport | null>(null);
  const confirmTitleId = useId();
  const confirmBodyId = useId();

  useEffect(() => {
    setActionError(initialErrorCode);
  }, [initialErrorCode]);

  const pageCount = Math.max(1, Math.ceil(keys.items.length / PAGE_SIZE));
  useEffect(() => {
    setPage((current) => Math.min(current, pageCount));
  }, [pageCount]);

  const visibleKeys = useMemo(
    () => keys.items.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE),
    [keys.items, page],
  );

  const refresh = async () => {
    if (refreshing) return;
    setRefreshing(true);
    setActionError(null);
    try {
      onKeysChange(await managerApi.apiConfigKeys());
      const current = await managerApi.apiConfigSession().catch(() => null);
      if (current) onSessionChange(current);
    } catch (cause) {
      const code = errorCode(cause) ?? "unknown";
      if (code === "orange_signed_out") {
        const signedOut = await managerApi.apiConfigSession().catch(() => ({
          ...session,
          authenticated: false,
          remembered: false,
          connection: "signed_out" as const,
          warning: code,
        }));
        onLogout({ ...signedOut, warning: signedOut.warning ?? code });
      } else {
        setActionError(code);
      }
    } finally {
      setRefreshing(false);
    }
  };

  const importCcs = async (keyId: number) => {
    if (keys.stale || importingId != null) return;
    setImportingId(keyId);
    setActionError(null);
    setCcsNotice(null);
    try {
      await managerApi.apiConfigImportCcs(keyId);
      setCcsNotice(keyId);
    } catch (cause) {
      setActionError(errorCode(cause) ?? "unknown");
    } finally {
      setImportingId(null);
    }
  };

  const writeLocal = async (key: ApiConfigKey) => {
    if (keys.stale || writingId != null) return;
    setConfirmKey(null);
    setWritingId(key.id);
    setActionError(null);
    setWriteReport(null);
    setRestartFailed(false);
    try {
      const report = await managerApi.apiConfigWriteLocal(key.id);
      setWriteReport(report);
      if (report.outcome === "committed") {
        onKeysChange({
          ...keys,
          items: keys.items.map((item) => ({ ...item, enabled: item.id === key.id })),
        });
        try {
          onKeysChange(await managerApi.apiConfigKeys());
        } catch {
          // Keep the verified local projection and success report visible.
        }
      }
    } catch (cause) {
      setActionError(errorCode(cause) ?? "unknown");
    } finally {
      setWritingId(null);
    }
  };

  const restart = async () => {
    if (restarting) return;
    setRestarting(true);
    setRestartFailed(false);
    try {
      await managerApi.apiConfigRestartCodex();
    } catch {
      setRestartFailed(true);
    } finally {
      setRestarting(false);
    }
  };

  const logout = async () => {
    if (loggingOut) return;
    setLoggingOut(true);
    setActionError(null);
    try {
      onLogout(await managerApi.apiConfigLogout());
    } catch (cause) {
      setActionError(errorCode(cause) ?? "unknown");
    } finally {
      setLoggingOut(false);
    }
  };

  const errorText = apiConfigErrorText(actionError, t);
  const persistentWarning = [
    "orange_credential_store",
    "orange_refresh_unavailable",
    "orange_persistence",
  ].includes(session.warning ?? "")
    ? apiConfigWarningText(session.warning, t)
    : null;
  const interrupted = keys.stale || session.connection === "interrupted";
  const reportMessage =
    writeReport?.outcome === "committed"
      ? t("config.writeSuccess")
      : writeReport?.outcome === "restored"
        ? t("config.writeRestored")
        : writeReport?.outcome === "recovery_required"
          ? t("config.recoveryRequired")
          : writeReport
            ? apiConfigErrorText(writeReport.errorCode, t)
            : null;
  const reportRole = writeReport?.outcome === "committed" ? "status" : "alert";

  return (
    <section className="api-config-connected api-config-with-footer">
      <header className="api-config-connection">
        <div className="api-config-connection-copy">
          <span className={`api-config-connection-state${interrupted ? " interrupted" : ""}`}>
            <span className={`dot${interrupted ? "" : " live"}`} aria-hidden="true" />
            {t(interrupted ? "config.interrupted" : "config.connected")}
          </span>
          <strong>{session.email}</strong>
          <span>{t("config.service")}</span>
        </div>
        <button
          className="iconbtn api-config-refresh"
          type="button"
          aria-label={t("config.refresh")}
          title={t("config.refresh")}
          onClick={() => void refresh()}
          disabled={refreshing}
        >
          <Icon name={refreshing ? "loader" : "refresh"} />
        </button>
      </header>

      {errorText ? (
        <div className="banner err" role="alert">
          <Icon name="alert" />
          <span>{errorText}</span>
        </div>
      ) : null}
      {persistentWarning ? (
        <div className="banner warn" role="status">
          <Icon name="alert" />
          <span>{persistentWarning}</span>
        </div>
      ) : null}
      {keys.stale ? (
        <div className="banner warn" role="status">
          <Icon name="alert" />
          <span>{t("config.stale")}</span>
        </div>
      ) : null}
      {ccsNotice != null ? (
        <div className="banner ok" role="status">
          <Icon name="link" />
          <span>{t("config.ccsSent")}</span>
        </div>
      ) : null}
      {writeReport && reportMessage ? (
        <div className={`api-config-write-report ${writeReport.outcome}`} role={reportRole}>
          <div className={`banner ${writeReport.outcome === "committed" ? "ok" : "err"}`}>
            <Icon name={writeReport.outcome === "committed" ? "check" : "alert"} />
            <span>{reportMessage}</span>
          </div>
          {writeReport.backupDir ? (
            <div className="api-config-backup">
              <span>{t("config.backupPath")}</span>
              <code>{writeReport.backupDir}</code>
            </div>
          ) : null}
          {writeReport.outcome === "committed" ? (
            <button className="btn primary api-config-restart" type="button" onClick={() => void restart()} disabled={restarting}>
              <Icon name={restarting ? "loader" : "play"} />
              {t(restarting ? "config.restarting" : "config.restart")}
            </button>
          ) : null}
          {restartFailed ? (
            <div className="banner err" role="alert">
              <Icon name="alert" />
              <span>{t("config.restartFailed")}</span>
            </div>
          ) : null}
        </div>
      ) : null}

      {keys.items.length === 0 ? (
        <div className="api-config-empty">
          <Icon name="key" />
          <span>{t("config.empty")}</span>
        </div>
      ) : (
        <ul className="api-key-list">
          {visibleKeys.map((key) => {
            const importing = importingId === key.id;
            const writing = writingId === key.id;
            const expiry = formatExpiry(key.expiresAt, lang, t("config.noExpiry"));
            const quota = key.quota <= 0
              ? t("config.unlimited")
              : t("config.quotaValue", { used: key.quotaUsed, quota: key.quota });
            return (
              <li className="api-key-row" data-testid={`api-key-${key.id}`} key={key.id}>
                <div className="api-key-heading">
                  <div>
                    <strong>{key.name}</strong>
                    <span>{key.groupName}</span>
                  </div>
                  <div className="api-key-tags">
                    {key.enabled ? <span className="tag enabled">{t("config.enabled")}</span> : null}
                    <span className={`tag api-key-status ${key.status}`}>{t(statusKey(key.status))}</span>
                  </div>
                </div>
                <code className="api-key-masked">{key.maskedKey}</code>
                <dl className="api-key-meta">
                  <div><dt>{t("config.quota")}</dt><dd>{quota}</dd></div>
                  <div><dt>{t("config.expiry")}</dt><dd>{expiry}</dd></div>
                </dl>
                <div className="api-key-actions">
                  <button
                    className="btn ghost"
                    type="button"
                    disabled={keys.stale || !key.actionable || importing}
                    onClick={() => void importCcs(key.id)}
                  >
                    <Icon name={importing ? "loader" : "link"} />
                    {t(importing ? "config.importingCcs" : "config.importCcs")}
                  </button>
                  <button
                    className="btn"
                    type="button"
                    disabled={keys.stale || !key.actionable || writing}
                    onClick={() => setConfirmKey(key)}
                  >
                    <Icon name={writing ? "loader" : "download"} />
                    {t(writing ? "config.writingLocal" : "config.writeLocal")}
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}

      {pageCount > 1 ? (
        <nav className="api-config-pagination" aria-label={t("config.page", { current: page, total: pageCount })}>
          <button className="btn ghost" type="button" onClick={() => setPage((value) => value - 1)} disabled={page === 1}>
            {t("config.previous")}
          </button>
          <span>{t("config.page", { current: page, total: pageCount })}</span>
          <button className="btn ghost" type="button" onClick={() => setPage((value) => value + 1)} disabled={page === pageCount}>
            {t("config.next")}
          </button>
        </nav>
      ) : null}

      <footer className="api-config-logout">
        <button className="btn ghost" type="button" onClick={() => void logout()} disabled={loggingOut}>
          <Icon name={loggingOut ? "loader" : "logOut"} />
          {t("config.logout")}
        </button>
      </footer>

      <Sheet
        open={confirmKey != null}
        onDismiss={() => setConfirmKey(null)}
        labelledBy={confirmTitleId}
        describedBy={confirmBodyId}
        initialFocus="primary"
        centeredInExpanded
      >
        <h3 id={confirmTitleId}>{t("config.confirm.title")}</h3>
        <div id={confirmBodyId}>
          <p>{t("config.confirm.body")}</p>
          <p>{t("config.confirm.closeWarning")}</p>
        </div>
        <div className="row2 sheet-actions">
          <button className="btn ghost" type="button" data-sheet-dismiss onClick={() => setConfirmKey(null)}>
            {t("config.confirm.cancel")}
          </button>
          <button
            className="btn primary"
            type="button"
            data-sheet-primary
            disabled={keys.stale}
            onClick={() => confirmKey && void writeLocal(confirmKey)}
          >
            {t("config.confirm.action")}
          </button>
        </div>
      </Sheet>
    </section>
  );
}
