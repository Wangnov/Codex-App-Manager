import { useEffect, useState } from "react";

import { errorCode, managerApi } from "../../services/managerApi";
import type { ApiConfigKeyList, ApiConfigSession } from "../../shared/types";
import { NavBar } from "../components";
import { Icon } from "../icons";
import { useI18n } from "../i18n";
import { ApiLoginForm } from "./apiConfig/ApiLoginForm";
import { ApiKeyList } from "./apiConfig/ApiKeyList";

const EMPTY_KEYS: ApiConfigKeyList = {
  items: [],
  stale: false,
  fetchedAtUnix: 0,
};

export function CodexConfig({ onBack }: { onBack: () => void }) {
  const { t } = useI18n();
  const [session, setSession] = useState<ApiConfigSession | null>(null);
  const [keys, setKeys] = useState<ApiConfigKeyList | null>(null);
  const [loginError, setLoginError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void managerApi
      .apiConfigSession()
      .then(async (restored) => {
        if (cancelled) return;
        setSession(restored);
        if (!restored.authenticated) return;
        try {
          const list = await managerApi.apiConfigKeys();
          if (!cancelled) setKeys(list);
        } catch (cause) {
          if (cancelled) return;
          const code = errorCode(cause) ?? "unknown";
          setLoginError(code);
          if (code === "orange_signed_out") {
            try {
              const current = await managerApi.apiConfigSession();
              if (!cancelled) {
                setSession(current);
                if (current.authenticated) setKeys(EMPTY_KEYS);
              }
            } catch {
              if (!cancelled) {
                setSession({
                  ...restored,
                  authenticated: false,
                  remembered: false,
                  connection: "signed_out",
                  warning: code,
                });
              }
            }
          } else {
            const current = await managerApi.apiConfigSession().catch(() => ({
              ...restored,
              connection: "interrupted" as const,
              warning: code,
            }));
            if (!cancelled) {
              setSession(current);
              setKeys(EMPTY_KEYS);
            }
          }
        }
      })
      .catch((cause) => {
        if (cancelled) return;
        setLoginError(errorCode(cause) ?? "unknown");
        setSession({
          authenticated: false,
          email: null,
          remembered: false,
          connection: "signed_out",
          warning: null,
        });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const login = async (email: string, password: string, remember: boolean) => {
    setLoginError(null);
    try {
      const connected = await managerApi.apiConfigLogin(email, password, remember);
      setSession(connected);
      try {
        setKeys(await managerApi.apiConfigKeys());
      } catch (cause) {
        const code = errorCode(cause) ?? "unknown";
        if (code === "orange_signed_out") {
          const current = await managerApi.apiConfigSession().catch(() => ({
            ...connected,
            authenticated: false,
            remembered: false,
            connection: "signed_out" as const,
            warning: code,
          }));
          setSession(current);
          if (current.authenticated) setKeys(EMPTY_KEYS);
        } else {
          setSession(
            await managerApi.apiConfigSession().catch(() => ({
              ...connected,
              connection: "interrupted" as const,
              warning: code,
            })),
          );
          setKeys(EMPTY_KEYS);
        }
        throw cause;
      }
    } catch (cause) {
      setLoginError(errorCode(cause) ?? "unknown");
      throw cause;
    }
  };

  return (
    <div className="pop api-config-view">
      <NavBar title={t("config.title")} onBack={onBack} />
      <div className="scroll scroll-wide view api-config-scroll">
        {session == null ? (
          <section className="api-config-restoring" role="status">
            <span className="api-config-login-mark" aria-hidden="true">
              <Icon name="loader" />
            </span>
            <h1>{t("config.restoring")}</h1>
            <p>{t("config.service")}</p>
          </section>
        ) : !session.authenticated ? (
          <ApiLoginForm
            email={session.email}
            errorCode={loginError}
            warning={session.warning}
            onLogin={login}
          />
        ) : keys ? (
          <ApiKeyList
            session={session}
            keys={keys}
            initialErrorCode={loginError}
            onKeysChange={setKeys}
            onSessionChange={setSession}
            onLogout={(signedOut) => {
              setSession(signedOut);
              setKeys(null);
              setLoginError(signedOut.warning);
            }}
          />
        ) : (
          <section className="api-config-restoring" role="status">
            <span className="api-config-login-mark" aria-hidden="true">
              <Icon name="loader" />
            </span>
            <h1>{t("config.refreshing")}</h1>
            <p>{session.email}</p>
          </section>
        )}
      </div>
    </div>
  );
}
