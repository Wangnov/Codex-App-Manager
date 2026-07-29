import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../../services/managerApi";
import type {
  ApiConfigKey,
  ApiConfigKeyList as ApiConfigKeyListModel,
  ApiConfigSession,
  ApiConfigWriteReport,
} from "../../../shared/types";
import { I18nProvider } from "../../i18n";
import { ThemeProvider } from "../../theme";
import { ApiKeyList } from "./ApiKeyList";

vi.mock("../../../services/managerApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../services/managerApi")>();
  return {
    ...actual,
    managerApi: {
      ...actual.managerApi,
      apiConfigSession: vi.fn(),
      apiConfigKeys: vi.fn(),
      apiConfigLogout: vi.fn(),
      apiConfigImportCcs: vi.fn(),
      apiConfigWriteLocal: vi.fn(),
      apiConfigRestartCodex: vi.fn(),
    },
  };
});

const api = vi.mocked(managerApi);

const SESSION: ApiConfigSession = {
  authenticated: true,
  email: "saved@example.com",
  remembered: true,
  connection: "connected",
  warning: null,
};

function activeKey(overrides: Partial<ApiConfigKey> = {}): ApiConfigKey {
  return {
    id: 1,
    name: "Primary",
    groupName: "OpenAI",
    maskedKey: "sk-a••••123",
    status: "active",
    quota: 100,
    quotaUsed: 12,
    expiresAt: null,
    actionable: true,
    enabled: false,
    ...overrides,
  };
}

function list(items: ApiConfigKey[], stale = false): ApiConfigKeyListModel {
  return { items, stale, fetchedAtUnix: 1 };
}

const WRITE_REPORT: ApiConfigWriteReport = {
  outcome: "committed",
  backupDir: "C:\\Users\\demo\\orangeapi-backups\\123",
  configPath: "C:\\Users\\demo\\.codex\\config.toml",
  authPath: "C:\\Users\\demo\\.codex\\auth.json",
  codexWasRunning: true,
  writeVerified: true,
  rollbackVerified: false,
  errorCode: null,
};

function renderKeyList(
  keys: ApiConfigKeyListModel,
  overrides: Partial<React.ComponentProps<typeof ApiKeyList>> = {},
) {
  const props: React.ComponentProps<typeof ApiKeyList> = {
    session: SESSION,
    keys,
    initialErrorCode: null,
    onKeysChange: vi.fn(),
    onSessionChange: vi.fn(),
    onLogout: vi.fn(),
    ...overrides,
  };
  render(
    <ThemeProvider>
      <I18nProvider>
        <ApiKeyList {...props} />
      </I18nProvider>
    </ThemeProvider>,
  );
  return props;
}

describe("ApiKeyList", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "en");
    api.apiConfigKeys.mockReset();
    api.apiConfigSession.mockReset();
    api.apiConfigLogout.mockReset();
    api.apiConfigImportCcs.mockReset();
    api.apiConfigWriteLocal.mockReset();
    api.apiConfigRestartCodex.mockReset();
    api.apiConfigKeys.mockResolvedValue(list([activeKey({ enabled: true })]));
    api.apiConfigSession.mockResolvedValue({
      ...SESSION,
      authenticated: false,
      connection: "signed_out",
      remembered: false,
    });
    api.apiConfigLogout.mockResolvedValue({ ...SESSION, authenticated: false, connection: "signed_out" });
    api.apiConfigImportCcs.mockResolvedValue(undefined);
    api.apiConfigWriteLocal.mockResolvedValue(WRITE_REPORT);
    api.apiConfigRestartCodex.mockResolvedValue(undefined);
  });

  it("marks the local key enabled and disables unusable actions", () => {
    renderKeyList(
      list([
        activeKey({ id: 1, enabled: true }),
        activeKey({ id: 2, name: "Disabled", status: "inactive", actionable: false }),
      ]),
    );

    const enabled = screen.getByTestId("api-key-1");
    expect(within(enabled).getByText("Enabled")).toBeInTheDocument();
    const inactive = screen.getByTestId("api-key-2");
    expect(within(inactive).getByRole("button", { name: "Import to CCS" })).toBeDisabled();
    expect(within(inactive).getByRole("button", { name: "Write to computer" })).toBeDisabled();
  });

  it("renders an invalid expiry value without crashing the key list", () => {
    renderKeyList(list([activeKey({ expiresAt: "not-a-date", actionable: false })]));

    expect(screen.getByText("not-a-date")).toBeInTheDocument();
  });

  it("keeps key actions stable and the logout footer outside list rows", () => {
    const { container } = render(
      <ThemeProvider>
        <I18nProvider>
          <ApiKeyList
            session={SESSION}
            keys={list([activeKey({ id: 1 }), activeKey({ id: 2 })])}
            initialErrorCode={null}
            onKeysChange={vi.fn()}
            onSessionChange={vi.fn()}
            onLogout={vi.fn()}
          />
        </I18nProvider>
      </ThemeProvider>,
    );

    expect(container.querySelectorAll(".api-key-actions")).toHaveLength(2);
    expect(container.querySelector(".api-key-list .api-config-logout")).toBeNull();
    expect(container.querySelector(".api-config-connected")).toHaveClass(
      "api-config-with-footer",
    );
  });

  it("paginates locally in stable 20-row pages", async () => {
    const user = userEvent.setup();
    const items = Array.from({ length: 21 }, (_, index) =>
      activeKey({ id: index + 1, name: `Key ${index + 1}` }),
    );
    renderKeyList(list(items));

    expect(screen.getAllByTestId(/^api-key-/)).toHaveLength(20);
    expect(screen.getByText("Page 1 of 2")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getAllByTestId(/^api-key-/)).toHaveLength(1);
    expect(screen.getByText("Key 21")).toBeInTheDocument();
  });

  it("retains the current list and reports a refresh failure", async () => {
    const user = userEvent.setup();
    api.apiConfigKeys.mockRejectedValue({ code: "orange_network", message: "secret backend text" });
    renderKeyList(list([activeKey()]));

    await user.click(screen.getByRole("button", { name: "Refresh" }));

    expect(screen.getByText("Primary")).toBeInTheDocument();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "OrangeAPI could not be reached. Check the network or proxy.",
    );
    expect(screen.queryByText("secret backend text")).not.toBeInTheDocument();
  });

  it("keeps secure-storage warnings visible after login transitions to the list", () => {
    renderKeyList(list([activeKey()]), {
      session: {
        ...SESSION,
        connection: "interrupted",
        warning: "orange_credential_store",
      },
    });

    expect(screen.getByText("Signed in, but the refresh token could not be saved securely."))
      .toBeInTheDocument();
  });

  it("syncs the connected session after a successful refresh", async () => {
    const user = userEvent.setup();
    const onSessionChange = vi.fn();
    api.apiConfigSession.mockResolvedValue(SESSION);
    renderKeyList(list([activeKey()], true), {
      session: { ...SESSION, connection: "interrupted", warning: "orange_network" },
      onSessionChange,
    });

    await user.click(screen.getByRole("button", { name: "Refresh" }));

    await waitFor(() => expect(onSessionChange).toHaveBeenCalledWith(SESSION));
  });

  it("returns to sign-in when a refresh reports an expired session", async () => {
    const user = userEvent.setup();
    const onLogout = vi.fn();
    api.apiConfigKeys.mockRejectedValue({
      code: "orange_signed_out",
      message: "backend text that must not be shown",
    });
    renderKeyList(list([activeKey()]), { onLogout });

    await user.click(screen.getByRole("button", { name: "Refresh" }));

    await waitFor(() => expect(onLogout).toHaveBeenCalledWith(expect.objectContaining({
      authenticated: false,
      email: "saved@example.com",
    })));
  });

  it("shows stale and empty states explicitly", () => {
    const { rerender } = render(
      <ThemeProvider>
        <I18nProvider>
          <ApiKeyList
            session={SESSION}
            keys={list([activeKey()], true)}
            initialErrorCode={null}
            onKeysChange={vi.fn()}
            onSessionChange={vi.fn()}
            onLogout={vi.fn()}
          />
        </I18nProvider>
      </ThemeProvider>,
    );
    expect(screen.getByText("Showing the last available list. Refresh failed.")).toBeInTheDocument();
    expect(screen.getByText("Connection interrupted")).toBeInTheDocument();
    const staleRow = screen.getByTestId("api-key-1");
    expect(within(staleRow).getByRole("button", { name: "Import to CCS" })).toBeDisabled();
    expect(within(staleRow).getByRole("button", { name: "Write to computer" })).toBeDisabled();

    rerender(
      <ThemeProvider>
        <I18nProvider>
          <ApiKeyList
            session={SESSION}
            keys={list([])}
            initialErrorCode={null}
            onKeysChange={vi.fn()}
            onSessionChange={vi.fn()}
            onLogout={vi.fn()}
          />
        </I18nProvider>
      </ThemeProvider>,
    );
    expect(screen.getByText("No OpenAI API keys")).toBeInTheDocument();
  });

  it("imports by key ID and keeps other key actions available while busy", async () => {
    const user = userEvent.setup();
    let finishImport!: () => void;
    api.apiConfigImportCcs.mockReturnValue(
      new Promise<void>((resolve) => {
        finishImport = resolve;
      }),
    );
    renderKeyList(list([activeKey({ id: 1 }), activeKey({ id: 2, name: "Secondary" })]));

    const first = screen.getByTestId("api-key-1");
    await user.click(within(first).getByRole("button", { name: "Import to CCS" }));
    expect(api.apiConfigImportCcs).toHaveBeenCalledWith(1);
    expect(within(screen.getByTestId("api-key-2")).getByRole("button", { name: "Write to computer" })).toBeEnabled();
    finishImport();
    expect(await screen.findByText("Sent to CC Switch")).toBeInTheDocument();
  });

  it("confirms replacement, writes by ID, then exposes restart and backup path", async () => {
    const user = userEvent.setup();
    api.apiConfigKeys.mockResolvedValue(list([activeKey({ enabled: true })]));
    renderKeyList(list([activeKey()]));

    await user.click(screen.getByRole("button", { name: "Write to computer" }));
    const dialog = screen.getByRole("dialog", { name: "Replace local Codex configuration?" });
    expect(dialog).toHaveTextContent("config.toml");
    expect(dialog).toHaveTextContent("auth.json");
    expect(dialog).toHaveTextContent("Codex");
    await user.click(within(dialog).getByRole("button", { name: "Back up and replace" }));

    expect(api.apiConfigWriteLocal).toHaveBeenCalledWith(1);
    expect(await screen.findByRole("button", { name: "Restart Codex" })).toBeVisible();
    expect(screen.getByText(WRITE_REPORT.backupDir!)).toBeVisible();
  });

  it.each([
    ["failed_before_mutation", "The local Codex files could not be updated."],
    ["restored", "The write failed. The original files were restored and verified."],
    ["recovery_required", "Automatic recovery failed. Restore the files from this backup."],
  ] as const)("reports the %s write outcome with its backup", async (outcome, message) => {
    const user = userEvent.setup();
    api.apiConfigWriteLocal.mockResolvedValue({
      ...WRITE_REPORT,
      outcome,
      writeVerified: false,
      rollbackVerified: outcome === "restored",
      errorCode: outcome === "failed_before_mutation" ? "provider_backup_failed" : "provider_io",
    });
    renderKeyList(list([activeKey()]));

    await user.click(screen.getByRole("button", { name: "Write to computer" }));
    await user.click(screen.getByRole("button", { name: "Back up and replace" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(message);
    expect(screen.getByText(WRITE_REPORT.backupDir!)).toBeVisible();
  });

  it("keeps restart available after a failed attempt", async () => {
    const user = userEvent.setup();
    api.apiConfigRestartCodex
      .mockRejectedValueOnce({ code: "internal_error", message: "no" })
      .mockResolvedValueOnce(undefined);
    renderKeyList(list([activeKey()]));
    await user.click(screen.getByRole("button", { name: "Write to computer" }));
    await user.click(screen.getByRole("button", { name: "Back up and replace" }));
    const restart = await screen.findByRole("button", { name: "Restart Codex" });

    await user.click(restart);
    expect(await screen.findByText("Codex could not be restarted. Try again.")).toBeInTheDocument();
    expect(restart).toBeEnabled();
    await user.click(restart);
    expect(api.apiConfigRestartCodex).toHaveBeenCalledTimes(2);
  });
});
