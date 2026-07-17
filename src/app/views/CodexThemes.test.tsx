import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../services/managerApi";
import { DEFAULT_SETTINGS, type CodexThemeStatusReport } from "../../shared/types";
import { I18nProvider } from "../i18n";
import { CodexThemes } from "./CodexThemes";

vi.mock("../../services/managerApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../services/managerApi")>();
  return {
    ...actual,
    managerApi: {
      codexThemeCatalog: vi.fn(),
      codexThemeList: vi.fn(),
      codexThemeStatus: vi.fn(),
      getSettings: vi.fn(),
    },
  };
});

const api = vi.mocked(managerApi);

const STATUS: CodexThemeStatusReport = {
  supported: true,
  activeTheme: null,
  daemon: null,
  cdpReady: false,
  codexRunning: false,
  nativeBackupPresent: false,
  storeDir: "C:\\themes",
  tryOnStash: false,
  recoveryRequired: false,
};

describe("CodexThemes empty state", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "zh-CN");
    api.codexThemeCatalog.mockResolvedValue([]);
    api.codexThemeList.mockResolvedValue([]);
    api.codexThemeStatus.mockResolvedValue(STATUS);
    api.getSettings.mockResolvedValue(DEFAULT_SETTINGS);
  });

  it("renders the sliders glyph inside the bounded hero medallion", async () => {
    const { container } = render(
      <I18nProvider>
        <CodexThemes onBack={vi.fn()} />
      </I18nProvider>,
    );

    await screen.findByText("还没有可用主题");
    expect(container.querySelector(".hero .ring.muted .cam-icon-sliders")).toBeInTheDocument();
    expect(container.querySelector(".hero > svg.ricon")).not.toBeInTheDocument();
  });
});
