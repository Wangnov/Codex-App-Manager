// Compact per-platform view of codex-app-mirror's release-manifest.json.
// Shared by the build-time snapshot (scripts/fetch-data.mjs) and the live
// status endpoint (worker/index.js), so both always agree on the shape.

export function summarizeManifest(m) {
  const mac = m?.sources?.macos ?? {};
  const win = m?.sources?.windows?.architectures ?? {};
  const sums = m?.derived?.latestChecksums ?? {};
  const macFile = (a) => (mac[a] ? { bytes: mac[a].contentLength ?? null, sha256: mac[a].sha256 ?? null } : null);
  const winFile = (a) => {
    const moniker = win[a]?.packageMoniker;
    return moniker ? { bytes: win[a].contentLength ?? null, sha256: sums[`${moniker}.Msix`] ?? null } : null;
  };
  return {
    version: m?.codexVersion ?? null,
    publishedAt: m?.publishedAt ?? null,
    files: {
      "mac-arm64": macFile("arm64"),
      "mac-intel": macFile("x64"),
      "win-x64": winFile("x64"),
      "win-arm64": winFile("arm64"),
    },
  };
}
