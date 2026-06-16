import type { Diagnostics } from "../shared/types";

export function formatDiagnostics(diagnostics: Diagnostics, jsError?: Error | null): string {
  const health = diagnostics.configHealth;
  const recentErrors = diagnostics.recentErrors.length
    ? diagnostics.recentErrors.map((line) => `- ${line}`).join("\n")
    : "- none";
  const lines = [
    "# Codex App Manager diagnostics",
    "",
    `Generated: ${new Date(diagnostics.generatedAtUnix * 1000).toISOString()}`,
    `Version: ${diagnostics.appVersion}`,
    `Platform: ${diagnostics.os}/${diagnostics.arch}`,
    `Update source: ${diagnostics.updateSource}`,
    `Custom source host: ${diagnostics.customSourceHost ?? "none"}`,
    `Windows install mode: ${diagnostics.windowsInstallMode ?? "n/a"}`,
    `Install status: ${diagnostics.installStatus}`,
    `Logs dir: ${diagnostics.logsDir ?? "n/a"}`,
    "",
    "## Config health",
    `Settings: ${health.settingsStatus}`,
    `Provenance: ${health.provenanceStatus}`,
    `Unknown source: ${health.unknownSource ?? "none"}`,
    `Detail: ${health.detail ?? "none"}`,
    "",
    "## Recent warnings/errors",
    recentErrors,
    "",
    "## Log tail",
    diagnostics.logTail || "(empty)",
  ];

  if (jsError) {
    lines.push(
      "",
      "## Frontend error",
      `${jsError.name}: ${jsError.message}`,
      jsError.stack ?? "(no stack)",
    );
  }

  return lines.join("\n");
}
