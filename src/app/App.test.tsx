import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";

vi.mock("./windowMode", () => ({
  WindowModeProvider: ({ children }: { children: ReactNode }) => children,
  useWindowModeOptional: () => ({
    mode: "expanded",
    switching: false,
    setMode: vi.fn(),
  }),
}));

vi.mock("./components", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./components")>();
  return { ...actual, QuitConfirm: () => null };
});

vi.mock("./views/Home", () => ({ Home: () => <main>Home view</main> }));
vi.mock("./views/Settings", () => ({ Settings: () => <main>Settings view</main> }));
vi.mock("./views/About", () => ({ About: () => null }));
vi.mock("./views/Uninstall", () => ({ Uninstall: () => null }));
vi.mock("./views/CodexConfig", () => ({ CodexConfig: () => <main>Config view</main> }));
vi.mock("./views/CodexThemes", () => ({ CodexThemes: () => <main>Themes view</main> }));

describe("App API configuration route", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "en");
  });

  it("keeps the config rail item active while the config view is open", async () => {
    const user = userEvent.setup();
    render(<App />);

    const config = screen.getByRole("button", { name: "API configuration" });
    await user.click(config);

    expect(config).toHaveAttribute("aria-current", "page");
    expect(screen.getByText("Config view")).toBeInTheDocument();
  });
});
