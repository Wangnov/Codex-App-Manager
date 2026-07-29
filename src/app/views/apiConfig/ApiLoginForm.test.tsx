import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { I18nProvider } from "../../i18n";
import { ApiLoginForm } from "./ApiLoginForm";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

function renderLogin(
  overrides: Partial<React.ComponentProps<typeof ApiLoginForm>> = {},
) {
  const props: React.ComponentProps<typeof ApiLoginForm> = {
    email: "saved@example.com",
    errorCode: null,
    warning: null,
    onLogin: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
  render(
    <I18nProvider>
      <ApiLoginForm {...props} />
    </I18nProvider>,
  );
  return props;
}

describe("ApiLoginForm", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "en");
  });

  it("prefills email, never prefills password, and submits the remember preference", async () => {
    const user = userEvent.setup();
    const onLogin = vi.fn().mockResolvedValue(undefined);
    renderLogin({ onLogin });

    expect(screen.getByLabelText("Email")).toHaveValue("saved@example.com");
    expect(screen.getByLabelText("Password")).toHaveValue("");
    await user.type(screen.getByLabelText("Password"), "pw");
    await user.click(screen.getByRole("checkbox", { name: "Remember sign-in" }));
    await user.click(screen.getByRole("button", { name: "Sign in" }));

    expect(onLogin).toHaveBeenCalledWith("saved@example.com", "pw", true);
    expect(screen.getByLabelText("Password")).toHaveValue("");
  });

  it("submits with Enter and toggles password visibility accessibly", async () => {
    const user = userEvent.setup();
    const onLogin = vi.fn().mockResolvedValue(undefined);
    renderLogin({ onLogin });
    const password = screen.getByLabelText("Password");

    expect(password).toHaveAttribute("type", "password");
    await user.click(screen.getByRole("button", { name: "Show password" }));
    expect(password).toHaveAttribute("type", "text");
    await user.type(password, "pw{Enter}");

    expect(onLogin).toHaveBeenCalledWith("saved@example.com", "pw", false);
  });

  it("locks duplicate submissions while a login is running", async () => {
    const user = userEvent.setup();
    const login = deferred<void>();
    const onLogin = vi.fn().mockReturnValue(login.promise);
    renderLogin({ onLogin });
    await user.type(screen.getByLabelText("Password"), "pw");

    const submit = screen.getByRole("button", { name: "Sign in" });
    await user.click(submit);
    expect(screen.getByRole("button", { name: "Signing in…" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Signing in…" }));
    expect(onLogin).toHaveBeenCalledOnce();

    login.resolve();
  });

  it("shows two-step verification separately from invalid credentials", () => {
    const { rerender } = render(
      <I18nProvider>
        <ApiLoginForm
          email={null}
          errorCode="orange_2fa_unsupported"
          warning={null}
          onLogin={vi.fn()}
        />
      </I18nProvider>,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Two-step verification is not supported in this version.",
    );

    rerender(
      <I18nProvider>
        <ApiLoginForm
          email={null}
          errorCode="orange_invalid_credentials"
          warning={null}
          onLogin={vi.fn()}
        />
      </I18nProvider>,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "The email or password is incorrect.",
    );
  });

  it("localizes secure-store and desktop-only warnings", () => {
    const { rerender } = render(
      <I18nProvider>
        <ApiLoginForm
          email={null}
          errorCode="desktop_required"
          warning="orange_credential_store"
          onLogin={vi.fn()}
        />
      </I18nProvider>,
    );

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Open the desktop app to use API configuration.",
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "Signed in, but the refresh token could not be saved securely.",
    );

    rerender(
      <I18nProvider>
        <ApiLoginForm
          email={null}
          errorCode={null}
          warning="orange_refresh_unavailable"
          onLogin={vi.fn()}
        />
      </I18nProvider>,
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "Signed in, but the refresh token could not be saved securely.",
    );
  });
});
