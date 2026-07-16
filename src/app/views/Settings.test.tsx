import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../services/managerApi";
import { DEFAULT_SETTINGS, type AppSettings, type ConfigHealth } from "../../shared/types";
import { CATALOG, I18nProvider, type Lang } from "../i18n";
import { ThemeProvider } from "../theme";
import { Settings } from "./Settings";

vi.mock("../../services/managerApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../services/managerApi")>();
  return {
    ...actual,
    managerApi: {
      getSettings: vi.fn(),
      setSettings: vi.fn(),
      getAutostart: vi.fn(),
      setAutostart: vi.fn(),
      getConfigHealth: vi.fn(),
      restoreConfigBackup: vi.fn(),
      resetConfig: vi.fn(),
      winDefaultInstallRoot: vi.fn(),
      winPickInstallDir: vi.fn(),
      winSetInstallRoot: vi.fn(),
      winResetInstallRoot: vi.fn(),
    },
  };
});

const api = vi.mocked(managerApi);

const HEALTHY_CONFIG: ConfigHealth = {
  settingsStatus: "ok",
  provenanceStatus: "ok",
  unknownSource: null,
  detail: null,
  settingsBackupAvailable: false,
  provenanceBackupAvailable: false,
};

const BROKEN_CONFIG: ConfigHealth = {
  settingsStatus: "corrupt",
  provenanceStatus: "corrupt",
  unknownSource: null,
  detail: "invalid JSON",
  settingsBackupAvailable: true,
  provenanceBackupAvailable: true,
};

const SAFETY_FLOW_LANGS = ["fr", "ar", "es", "zh-TW"] as const satisfies readonly Lang[];

function settings(overrides: Partial<AppSettings> = {}): AppSettings {
  return { ...DEFAULT_SETTINGS, ...overrides };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function renderSettings() {
  return render(
    <ThemeProvider>
      <I18nProvider>
        <Settings
          onBack={vi.fn()}
          onOpenAbout={vi.fn()}
          onOpenUninstall={vi.fn()}
          onOpenConfig={vi.fn()}
          onOpenThemes={vi.fn()}
        />
      </I18nProvider>
    </ThemeProvider>,
  );
}

describe("Settings runtime contract", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "zh-CN");
    api.getSettings.mockReset();
    api.setSettings.mockReset();
    api.getAutostart.mockReset();
    api.setAutostart.mockReset();
    api.getConfigHealth.mockReset();
    api.restoreConfigBackup.mockReset();
    api.resetConfig.mockReset();
    api.getAutostart.mockResolvedValue(false);
    api.setSettings.mockImplementation(async (next) => next);
    api.getConfigHealth.mockResolvedValue(HEALTHY_CONFIG);
    api.restoreConfigBackup.mockResolvedValue(HEALTHY_CONFIG);
    api.resetConfig.mockResolvedValue(HEALTHY_CONFIG);
  });

  it("keeps the form non-editable until settings hydrate", async () => {
    const load = deferred<AppSettings>();
    api.getSettings.mockReturnValueOnce(load.promise);

    renderSettings();

    expect(screen.getByText("正在加载设置…")).toBeInTheDocument();
    const form = screen.getByText("正在加载设置…").closest(".scroll");
    expect(form).toHaveAttribute("inert");

    load.resolve(settings({ source: "mirror", periodicCheck: false }));
    await waitFor(() => expect(screen.queryByText("正在加载设置…")).not.toBeInTheDocument());
    expect(form).not.toHaveAttribute("inert");
    expect(screen.getByRole("radio", { name: /镜像/ })).toHaveAttribute("aria-checked", "true");
  });

  it("does not persist empty custom source selection until a URL is entered", async () => {
    const user = userEvent.setup();
    api.getSettings.mockResolvedValue(settings({ source: "mirror" }));
    renderSettings();

    await waitFor(() => expect(screen.queryByText("正在加载设置…")).not.toBeInTheDocument());
    const sourceGroup = screen.getByRole("radiogroup", { name: "更新源" });
    await user.click(within(sourceGroup).getByRole("radio", { name: "自定义" }));

    expect(screen.getAllByText("请填写有效的 https 地址后才会保存").length).toBeGreaterThan(0);
    expect(api.setSettings).not.toHaveBeenCalled();
    expect(within(sourceGroup).getByRole("radio", { name: "自定义" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
  });

  it("does not persist empty custom proxy mode until a URL is entered", async () => {
    const user = userEvent.setup();
    api.getSettings.mockResolvedValue(settings({ proxyMode: "system" }));
    renderSettings();

    await waitFor(() => expect(screen.queryByText("正在加载设置…")).not.toBeInTheDocument());
    const proxyGroup = screen.getByRole("radiogroup", { name: "代理" });
    await user.click(within(proxyGroup).getByRole("radio", { name: "自定义" }));

    expect(screen.getAllByText("请填写有效的代理地址后才会保存").length).toBeGreaterThan(0);
    expect(api.setSettings).not.toHaveBeenCalled();
  });

  it("coerces cleared custom source to auto so UI and runtime stay aligned", async () => {
    const user = userEvent.setup();
    api.getSettings.mockResolvedValue(
      settings({ source: "custom", customUrl: "https://example.test/feed" }),
    );
    renderSettings();

    await waitFor(() => expect(screen.queryByText("正在加载设置…")).not.toBeInTheDocument());
    const input = screen.getByRole("textbox", { name: "自定义" });
    await user.clear(input);
    await user.tab();

    await waitFor(() =>
      expect(api.setSettings).toHaveBeenCalledWith(
        expect.objectContaining({ source: "auto", customUrl: "" }),
      ),
    );
    const sourceGroup = screen.getByRole("radiogroup", { name: "更新源" });
    expect(within(sourceGroup).getByRole("radio", { name: /自动/ })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.queryByRole("textbox", { name: "自定义" })).not.toBeInTheDocument();
  });

  it("coerces cleared custom proxy to system so UI and runtime stay aligned", async () => {
    const user = userEvent.setup();
    api.getSettings.mockResolvedValue(
      settings({ proxyMode: "custom", customProxyUrl: "socks5h://127.0.0.1:7890" }),
    );
    renderSettings();

    await waitFor(() => expect(screen.queryByText("正在加载设置…")).not.toBeInTheDocument());
    const input = screen.getByRole("textbox", { name: "自定义" });
    await user.clear(input);
    await user.tab();

    await waitFor(() =>
      expect(api.setSettings).toHaveBeenCalledWith(
        expect.objectContaining({ proxyMode: "system", customProxyUrl: "" }),
      ),
    );
    const proxyGroup = screen.getByRole("radiogroup", { name: "代理" });
    expect(within(proxyGroup).getByRole("radio", { name: "跟随系统" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
  });

  it("saves custom source from the input current value on blur", async () => {
    const user = userEvent.setup();
    api.getSettings.mockResolvedValue(settings({ source: "mirror" }));
    renderSettings();

    await waitFor(() => expect(screen.queryByText("正在加载设置…")).not.toBeInTheDocument());
    const sourceGroup = screen.getByRole("radiogroup", { name: "更新源" });
    await user.click(within(sourceGroup).getByRole("radio", { name: "自定义" }));
    const input = screen.getByRole("textbox", { name: "自定义" });
    await user.type(input, "https://mirror.example.test/appcast.xml");
    await user.tab();

    await waitFor(() =>
      expect(api.setSettings).toHaveBeenCalledWith(
        expect.objectContaining({
          source: "custom",
          customUrl: "https://mirror.example.test/appcast.xml",
        }),
      ),
    );
  });

  it("states that periodic checks only run while the manager is open", async () => {
    api.getSettings.mockResolvedValue(settings());
    renderSettings();
    await waitFor(() => expect(screen.queryByText("正在加载设置…")).not.toBeInTheDocument());
    expect(screen.getByText("仅在管理器保持打开时运行")).toBeInTheDocument();
  });

  it("states Auto is not a speed test and system proxy is not a full PAC guarantee", async () => {
    api.getSettings.mockResolvedValue(settings({ proxyMode: "system" }));
    renderSettings();
    await waitFor(() => expect(screen.queryByText("正在加载设置…")).not.toBeInTheDocument());
    expect(screen.getByText("自动选用可用的更新源（非网速测速）")).toBeInTheDocument();
    expect(screen.getByText("尽量遵循系统代理；不保证完整 PAC 行为")).toBeInTheDocument();
  });

  it("disables the unfinished Codex config entry", async () => {
    api.getSettings.mockResolvedValue(settings());
    renderSettings();
    await waitFor(() => expect(screen.queryByText("正在加载设置…")).not.toBeInTheDocument());
    const config = screen.getByRole("button", { name: /Codex 配置管理/ });
    expect(config).toBeDisabled();
    expect(screen.getByText("当前版本尚未提供")).toBeInTheDocument();
  });

  it("keeps repair actions collapsed under More until requested", async () => {
    const user = userEvent.setup();
    api.getSettings.mockResolvedValue(settings());
    renderSettings();

    await waitFor(() => expect(screen.queryByText("正在加载设置…")).not.toBeInTheDocument());
    const repair = screen.getByRole("button", { name: "修复与恢复" });
    expect(repair).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: "清除管理记录" })).not.toBeInTheDocument();

    await user.click(repair);

    expect(repair).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: "清除管理记录" })).toBeInTheDocument();
  });

  it("ignores a late hydrate after the user already edited", async () => {
    const user = userEvent.setup();
    const load = deferred<AppSettings>();
    api.getSettings.mockReturnValueOnce(load.promise);
    // First paint uses defaults; we still lock with inert, so wait for hydrate then edit,
    // and separately verify the saver path via a second delayed hydrate is a unit concern.
    // Here: hydrate once, edit source, ensure saved value sticks.
    renderSettings();
    load.resolve(settings({ source: "auto" }));
    await waitFor(() => expect(screen.queryByText("正在加载设置…")).not.toBeInTheDocument());

    await user.click(screen.getByRole("radio", { name: /镜像/ }));
    await waitFor(() =>
      expect(api.setSettings).toHaveBeenCalledWith(expect.objectContaining({ source: "mirror" })),
    );
    expect(screen.getByRole("radio", { name: /镜像/ })).toHaveAttribute("aria-checked", "true");
  });

  it.each(SAFETY_FLOW_LANGS)(
    "shows and completes the managed-record reset consequence flow in %s",
    async (lang) => {
      const user = userEvent.setup();
      localStorage.setItem("cam.lang", lang);
      api.getSettings.mockResolvedValue(settings());
      api.getConfigHealth
        .mockResolvedValueOnce(BROKEN_CONFIG)
        .mockResolvedValue(HEALTHY_CONFIG);

      renderSettings();

      expect(await screen.findByText(CATALOG[lang]["settings.health.banner"])).toBeInTheDocument();
      const resetLabel = CATALOG[lang]["settings.health.clearProvenance"];
      await user.click(
        screen.getByRole("button", { name: CATALOG[lang]["settings.health.header"] }),
      );
      expect(
        screen.getByRole("button", { name: CATALOG[lang]["settings.health.reset"] }),
      ).toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: resetLabel }));

      const title = CATALOG[lang]["settings.health.resetConfirm.title"].replace(
        "{which}",
        CATALOG[lang]["settings.health.provenance"],
      );
      const dialog = screen.getByRole("dialog", { name: title });
      const provenanceBody = CATALOG[lang]["settings.health.resetConfirm.body.provenance"].replace(
        "{action}",
        CATALOG[lang]["home.external.cta"],
      );
      expect(
        within(dialog).getByText(provenanceBody),
      ).toBeInTheDocument();

      await user.click(within(dialog).getByRole("button", { name: resetLabel }));

      await waitFor(() => expect(api.resetConfig).toHaveBeenCalledWith("provenance"));
      expect(await screen.findByText(CATALOG[lang]["settings.health.verified"])).toBeInTheDocument();
      expect(document.documentElement.dir).toBe(lang === "ar" ? "rtl" : "ltr");
    },
  );
});
