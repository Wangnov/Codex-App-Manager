import type {
  MacUpdateReport,
  SkippedCodexUpdate,
  WindowsUpdatePlan,
} from "../shared/types";

export function macSkippedUpdateCandidate(
  report: MacUpdateReport | null,
): SkippedCodexUpdate | null {
  const plan = report?.plan ?? null;
  if (!plan || plan.upToDate) return null;
  const version = plan.latestShortVersion || `build ${plan.latestBuild}`;
  return {
    platform: "macos",
    target: `macos:${plan.latestBuild}`,
    version,
    skippedAt: Date.now(),
  };
}

export function winSkippedUpdateCandidate(
  plan: WindowsUpdatePlan | null,
): SkippedCodexUpdate | null {
  if (!plan || plan.upToDate) return null;
  const target = plan.packageMoniker || plan.latestVersion;
  if (!target || !plan.latestVersion) return null;
  return {
    platform: "windows",
    target: `windows:${target}`,
    version: plan.latestVersion,
    skippedAt: Date.now(),
  };
}

export function skippedUpdateMatches(
  saved: SkippedCodexUpdate | null | undefined,
  candidate: SkippedCodexUpdate | null,
): boolean {
  return Boolean(
    saved &&
      candidate &&
      saved.platform === candidate.platform &&
      saved.target === candidate.target,
  );
}
