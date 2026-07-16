import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it } from "vitest";

import { TopBar } from "./components";
import { useI18n, I18nProvider } from "./i18n";
import { acquireNavLock } from "./navLock";
import { Rail, type RailSection } from "./Rail";
import { ThemeProvider } from "./theme";
import { WindowModeProvider } from "./windowMode";

// The full chrome a mode switch touches: the TopBar (owns the expand control)
// plus the Rail (owns collapse + navigation). jsdom has no Tauri runtime, so
// switches exercise the browser-fallback path: CSS/dataset only, no native
// resize — exactly what the browser preview does.
function Harness() {
  const [section, setSection] = useState<RailSection>("home");
  return (
    <ThemeProvider>
      <I18nProvider>
        <WindowModeProvider>
          <TopBar />
          <Rail section={section} onNavigate={setSection} />
          <ActiveSection section={section} />
        </WindowModeProvider>
      </I18nProvider>
    </ThemeProvider>
  );
}

function ActiveSection({ section }: { section: RailSection }) {
  return <output data-testid="active-section">{section}</output>;
}

// Chrome that reacts to the window mode must also survive *without* the
// provider (isolated component renders elsewhere in the suite).
function Bare() {
  return (
    <ThemeProvider>
      <I18nProvider>
        <TopBar />
        <Rail section="home" onNavigate={() => undefined} />
      </I18nProvider>
    </ThemeProvider>
  );
}

async function expand() {
  fireEvent.click(screen.getByTitle(/expand workspace/i));
  await waitFor(() =>
    expect(document.documentElement.dataset.windowMode).toBe("expanded"),
  );
}

describe("window modes", () => {
  it("starts compact: mode stamped on <html>, rail hidden, expand offered", () => {
    render(<Harness />);
    expect(document.documentElement.dataset.windowMode).toBe("compact");
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
    expect(screen.getByTitle(/expand workspace/i)).toBeInTheDocument();
  });

  it("expanding shows the rail, retires the expand control and remembers the size", async () => {
    render(<Harness />);
    await expand();
    expect(screen.getByRole("navigation")).toBeInTheDocument();
    expect(screen.queryByTitle(/expand workspace/i)).not.toBeInTheDocument();
    // The echoed (browser-fallback default) size is persisted for next time.
    expect(JSON.parse(localStorage.getItem("cam.windowSize.expanded") ?? "null")).toEqual({
      width: 1100,
      height: 720,
    });
  });

  it("rail navigation reports sections and marks the active one", async () => {
    render(<Harness />);
    await expand();
    const settings = screen.getByRole("button", { name: /^settings$/i });
    fireEvent.click(settings);
    expect(screen.getByTestId("active-section")).toHaveTextContent("settings");
    expect(settings).toHaveAttribute("aria-current", "page");
  });

  it("rail collapse returns to compact and removes the rail", async () => {
    render(<Harness />);
    await expand();
    fireEvent.click(screen.getByRole("button", { name: /collapse workspace/i }));
    await waitFor(() =>
      expect(document.documentElement.dataset.windowMode).toBe("compact"),
    );
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
  });

  it("renders no mode chrome without a provider", () => {
    render(<Bare />);
    expect(screen.queryByTitle(/expand workspace/i)).not.toBeInTheDocument();
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
  });

  it("nav lock disables rail navigation but not collapse", async () => {
    render(<Harness />);
    await expand();
    // An in-flight operation (ProgressScreen) takes the lock — the rail's
    // exits must close so no concurrent operation can start, while collapsing
    // the window (shape only, view untouched) stays available.
    const release = acquireNavLock();
    try {
      await waitFor(() =>
        expect(screen.getByRole("button", { name: /^home$/i })).toBeDisabled(),
      );
      expect(screen.getByRole("button", { name: /^settings$/i })).toBeDisabled();
      expect(screen.getByRole("button", { name: /collapse workspace/i })).toBeEnabled();
    } finally {
      release();
    }
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /^home$/i })).toBeEnabled(),
    );
  });
});

// Guard the i18n contract: TopBar itself resolves the tooltip through t(), so
// a missing key would render the raw key string.
function Tooltip() {
  const { t } = useI18n();
  return <span>{t("nav.expand")}</span>;
}

describe("window mode copy", () => {
  it("resolves nav.expand", () => {
    render(
      <ThemeProvider>
        <I18nProvider>
          <Tooltip />
        </I18nProvider>
      </ThemeProvider>,
    );
    expect(screen.getByText(/expand workspace/i)).toBeInTheDocument();
  });
});
