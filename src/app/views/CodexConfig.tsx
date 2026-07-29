import { useEffect, useState } from "react";

import { errorCode, managerApi } from "../../services/managerApi";
import type { ApiConfigKeyList, ApiConfigSession } from "../../shared/types";
import { NavBar } from "../components";
import { Icon } from "../icons";
import { useI18n } from "../i18n";
import { ApiLoginForm } from "./apiConfig/ApiLoginForm";

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
          if (!cancelled) setLoginError(errorCode(cause) ?? "unknown");
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
      const list = await managerApi.apiConfigKeys();
      setKeys(list);
    } catch (cause) {
      setLoginError(errorCode(cause) ?? "unknown");
      throw cause;
    }
  };

  return (
    <div className="pop api-config-view">
      <NavBar title={t("config.title")} onBack={onBack} />
      <div className="scroll view api-config-scroll">
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
        ) : (
          <section className="api-config-connected" aria-live="polite">
            <div className="banner ok" role="status">
              <Icon name="link" />
              <span>{t("config.connected")}</span>
            </div>
            <span className="api-config-connected-email">{session.email}</span>
            {keys ? <span className="api-config-key-count">{keys.items.length}</span> : null}
          </section>
        )}
      </div>
    </div>
  );
}
