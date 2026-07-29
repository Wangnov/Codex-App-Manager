import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../services/managerApi";
import type { ApiConfigKeyList, ApiConfigSession } from "../../shared/types";
import { I18nProvider } from "../i18n";
import { ThemeProvider } from "../theme";
import { CodexConfig } from "./CodexConfig";

vi.mock("../../services/managerApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../services/managerApi")>();
  return {
    ...actual,
    managerApi: {
      ...actual.managerApi,
      apiConfigSession: vi.fn(),
      apiConfigLogin: vi.fn(),
      apiConfigKeys: vi.fn(),
    },
  };
});

const api = vi.mocked(managerApi);

const SIGNED_OUT: ApiConfigSession = {
  authenticated: false,
  email: "saved@example.com",
  remembered: false,
  connection: "signed_out",
  warning: null,
};

const CONNECTED: ApiConfigSession = {
  authenticated: true,
  email: "saved@example.com",
  remembered: true,
  connection: "connected",
  warning: null,
};

const KEYS: ApiConfigKeyList = {
  items: [],
  stale: false,
  fetchedAtUnix: 1,
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

function renderConfig() {
  return render(
    <ThemeProvider>
      <I18nProvider>
        <CodexConfig onBack={vi.fn()} />
      </I18nProvider>
    </ThemeProvider>,
  );
}

describe("CodexConfig signed-out orchestration", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "en");
    api.apiConfigSession.mockReset();
    api.apiConfigLogin.mockReset();
    api.apiConfigKeys.mockReset();
  });

  it("shows a stable restoring state before deciding which view to render", async () => {
    const restore = deferred<ApiConfigSession>();
    api.apiConfigSession.mockReturnValue(restore.promise);
    renderConfig();

    expect(screen.getByText("Checking saved sign-in…")).toBeInTheDocument();
    expect(screen.queryByLabelText("Email")).not.toBeInTheDocument();

    restore.resolve(SIGNED_OUT);
    expect(await screen.findByLabelText("Email")).toHaveValue("saved@example.com");
  });

  it("logs in, immediately loads keys, and transitions to the connected view", async () => {
    const user = userEvent.setup();
    api.apiConfigSession.mockResolvedValue(SIGNED_OUT);
    api.apiConfigLogin.mockResolvedValue(CONNECTED);
    api.apiConfigKeys.mockResolvedValue(KEYS);
    renderConfig();

    await user.type(await screen.findByLabelText("Password"), "pw");
    await user.click(screen.getByRole("checkbox", { name: "Remember sign-in" }));
    await user.click(screen.getByRole("button", { name: "Sign in" }));

    await waitFor(() =>
      expect(api.apiConfigLogin).toHaveBeenCalledWith("saved@example.com", "pw", true),
    );
    expect(api.apiConfigKeys).toHaveBeenCalledOnce();
    expect(await screen.findByText("Connected")).toBeInTheDocument();
  });

  it("renders stable localized login errors instead of backend messages", async () => {
    const user = userEvent.setup();
    api.apiConfigSession.mockResolvedValue(SIGNED_OUT);
    api.apiConfigLogin.mockRejectedValue({
      code: "orange_invalid_credentials",
      message: "backend text that must not be shown",
    });
    renderConfig();

    await user.type(await screen.findByLabelText("Password"), "wrong");
    await user.click(screen.getByRole("button", { name: "Sign in" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The email or password is incorrect.",
    );
    expect(screen.queryByText("backend text that must not be shown")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Password")).toHaveValue("");
  });
});
