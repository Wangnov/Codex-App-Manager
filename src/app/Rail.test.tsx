import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { I18nProvider } from "./i18n";
import { Rail } from "./Rail";
import { ThemeProvider } from "./theme";

const setMode = vi.fn();

vi.mock("./windowMode", () => ({
  useWindowModeOptional: () => ({ mode: "expanded", switching: false, setMode }),
}));

function renderExpandedRail(onNavigate = vi.fn()) {
  render(
    <ThemeProvider>
      <I18nProvider>
        <Rail section="home" onNavigate={onNavigate} />
      </I18nProvider>
    </ThemeProvider>,
  );
  return onNavigate;
}

describe("Rail API configuration entry", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "en");
  });

  it("places API configuration immediately below Home", () => {
    renderExpandedRail();

    const nav = screen.getByRole("navigation", { name: "Navigation" });
    expect(within(nav).getAllByRole("button").map((button) => button.textContent)).toEqual([
      "Home",
      "API configuration",
      "Skins",
      "Settings",
    ]);
  });

  it("navigates to the config section by its key", async () => {
    const user = userEvent.setup();
    const onNavigate = renderExpandedRail();

    await user.click(screen.getByRole("button", { name: "API configuration" }));

    expect(onNavigate).toHaveBeenCalledWith("config");
  });
});
