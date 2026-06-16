import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Diagnostics } from "../shared/types";
import { managerApi } from "../services/managerApi";
import { ErrorBoundary } from "./ErrorBoundary";

vi.mock("../services/managerApi", () => ({
  managerApi: {
    getDiagnostics: vi.fn(),
    reportFrontendError: vi.fn(() => Promise.resolve()),
  },
}));

const diagnostics: Diagnostics = {
  appVersion: "0.1.17",
  os: "macos",
  arch: "aarch64",
  locale: null,
  updateSource: "auto",
  customSourceHost: null,
  windowsInstallMode: null,
  installStatus: "macos status=managed",
  configHealth: {
    settingsStatus: "ok",
    provenanceStatus: "ok",
    unknownSource: null,
    detail: null,
  },
  logsDir: "/tmp/logs",
  recentErrors: [],
  logTail: "",
  generatedAtUnix: 1,
};

function Boom(): never {
  throw new Error("boom");
}

const getDiagnostics = vi.mocked(managerApi.getDiagnostics);
const reportFrontendError = vi.mocked(managerApi.reportFrontendError);

beforeEach(() => {
  localStorage.setItem("cam.lang", "en");
  getDiagnostics.mockResolvedValue(diagnostics);
});

describe("ErrorBoundary", () => {
  it("renders a crash screen and copies diagnostics with the JS error", async () => {
    const user = userEvent.setup();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const writeText = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue(undefined);

    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>,
    );

    expect(screen.getByText("Something went wrong")).toBeInTheDocument();
    expect(screen.getByText("Error: boom")).toBeInTheDocument();
    expect(reportFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "render", message: "boom" }),
    );

    await user.click(screen.getByRole("button", { name: "Copy diagnostics" }));
    await waitFor(() => expect(writeText).toHaveBeenCalled());
    expect(writeText.mock.calls[0][0]).toContain("## Frontend error");
    expect(screen.getByRole("button", { name: "Diagnostics copied" })).toBeInTheDocument();

    writeText.mockRestore();
    consoleError.mockRestore();
  });

  it("shows the same crash screen for cam:fatal events", async () => {
    render(
      <ErrorBoundary>
        <div>healthy</div>
      </ErrorBoundary>,
    );

    window.dispatchEvent(new CustomEvent("cam:fatal", { detail: { error: new Error("fatal") } }));

    await waitFor(() => expect(screen.getByText("Something went wrong")).toBeInTheDocument());
    expect(screen.getByText("Error: fatal")).toBeInTheDocument();
  });
});
