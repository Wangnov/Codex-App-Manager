import { describe, expect, it } from "vitest";

import type { MacUpdateReport, WindowsUpdatePlan } from "../shared/types";
import {
  macSkippedUpdateCandidate,
  skippedUpdateMatches,
  winSkippedUpdateCandidate,
} from "./skippedUpdate";

describe("skipped update helpers", () => {
  it("matches only the skipped macOS build", () => {
    const report = {
      plan: {
        upToDate: false,
        latestBuild: 3575,
        latestShortVersion: "26.602.30954",
      },
    } as MacUpdateReport;

    const skipped = macSkippedUpdateCandidate(report);

    expect(skippedUpdateMatches(skipped, macSkippedUpdateCandidate(report))).toBe(true);
    expect(
      skippedUpdateMatches(skipped, macSkippedUpdateCandidate({
        plan: {
          upToDate: false,
          latestBuild: 3600,
          latestShortVersion: "26.602.36000",
        },
      } as MacUpdateReport)),
    ).toBe(false);
  });

  it("matches only the skipped Windows package moniker", () => {
    const plan = {
      upToDate: false,
      latestVersion: "26.623.31921.0",
      packageMoniker: "OpenAI.Codex_26.623.31921.0_x64__2p2nqsd0c76g0",
    } as WindowsUpdatePlan;

    const skipped = winSkippedUpdateCandidate(plan);

    expect(skippedUpdateMatches(skipped, winSkippedUpdateCandidate(plan))).toBe(true);
    expect(
      skippedUpdateMatches(skipped, winSkippedUpdateCandidate({
        upToDate: false,
        latestVersion: "26.623.40000.0",
        packageMoniker: "OpenAI.Codex_26.623.40000.0_x64__2p2nqsd0c76g0",
      } as WindowsUpdatePlan)),
    ).toBe(false);
  });
});
