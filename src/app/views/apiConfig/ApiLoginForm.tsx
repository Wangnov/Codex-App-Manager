import { useState, type FormEvent } from "react";

import { Icon } from "../../icons";
import { useI18n } from "../../i18n";
import { apiConfigErrorText, apiConfigWarningText } from "./errors";

export function ApiLoginForm({
  email,
  errorCode,
  warning,
  onLogin,
}: {
  email: string | null;
  errorCode: string | null;
  warning: string | null;
  onLogin: (email: string, password: string, remember: boolean) => Promise<void>;
}) {
  const { t } = useI18n();
  const [emailValue, setEmailValue] = useState(email ?? "");
  const [password, setPassword] = useState("");
  const [remember, setRemember] = useState(false);
  const [showPassword, setShowPassword] = useState(false);
  const [busy, setBusy] = useState(false);
  const error = apiConfigErrorText(errorCode, t);
  const warningText = apiConfigWarningText(warning, t);

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    try {
      await onLogin(emailValue.trim(), password, remember);
    } catch {
      // The owner maps the stable command code to localized copy.
    } finally {
      setPassword("");
      setBusy(false);
    }
  };

  return (
    <section className="api-config-login" aria-labelledby="api-config-login-title">
      <div className="api-config-login-heading">
        <span className="api-config-login-mark" aria-hidden="true">
          <Icon name="key" />
        </span>
        <div>
          <h1 id="api-config-login-title">{t("config.signedOut")}</h1>
          <p>{t("config.service")}</p>
        </div>
      </div>

      {error ? (
        <div className="banner err" role="alert">
          <Icon name="alert" />
          <span>{error}</span>
        </div>
      ) : null}
      {warningText ? (
        <div className="banner warn" role="status">
          <Icon name="shield" />
          <span>{warningText}</span>
        </div>
      ) : null}

      <form className="api-config-login-form" onSubmit={(event) => void submit(event)}>
        <label className="api-config-field">
          <span>{t("config.email")}</span>
          <input
            className="input"
            type="email"
            autoComplete="username"
            value={emailValue}
            onChange={(event) => setEmailValue(event.currentTarget.value)}
            disabled={busy}
            required
          />
        </label>
        <label className="api-config-field">
          <span>{t("config.password")}</span>
          <span className="api-config-password">
            <input
              className="input"
              type={showPassword ? "text" : "password"}
              autoComplete="current-password"
              value={password}
              onChange={(event) => setPassword(event.currentTarget.value)}
              disabled={busy}
              required
            />
            <button
              className="api-config-password-toggle"
              type="button"
              aria-label={t(showPassword ? "config.hidePassword" : "config.showPassword")}
              title={t(showPassword ? "config.hidePassword" : "config.showPassword")}
              onClick={() => setShowPassword((visible) => !visible)}
              disabled={busy}
            >
              <Icon name={showPassword ? "eyeOff" : "eye"} />
            </button>
          </span>
        </label>
        <label className="api-config-remember">
          <input
            type="checkbox"
            checked={remember}
            onChange={(event) => setRemember(event.currentTarget.checked)}
            disabled={busy}
          />
          <span>{t("config.remember")}</span>
        </label>
        <button className="btn primary api-config-login-submit" type="submit" disabled={busy}>
          <Icon name={busy ? "loader" : "key"} />
          {t(busy ? "config.loggingIn" : "config.login")}
        </button>
      </form>
    </section>
  );
}
