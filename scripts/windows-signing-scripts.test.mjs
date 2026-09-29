import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const signScript = join(repoRoot, "scripts/sign-windows-authenticode.ps1");

// Scripts a contributor's Windows PowerShell 5.1 (`powershell`) may execute.
// 5.1 reads BOM-less files as the ANSI code page, so a single non-ASCII
// character inside a string (an em dash is enough: its UTF-8 bytes decode to
// a curly quote that terminates the string) breaks parsing there while pwsh 7
// parses the same file fine. Keep these ASCII-only (or, if that ever has to
// change, ship a UTF-8 BOM).
const signingScripts = [
  "scripts/sign-windows-authenticode.ps1",
  "scripts/verify-windows-authenticode.ps1",
  "scripts/resolve-esigner-cka-installer.ps1",
];

function nonAsciiLines(bytes) {
  const lines = [];
  let line = 1;
  for (const b of bytes) {
    if (b === 0x0a) line += 1;
    else if (b > 0x7f && !lines.includes(line)) lines.push(line);
  }
  return lines;
}

describe("Windows signing scripts encoding", () => {
  it("keeps every signing script ASCII-only so PS 5.1 parses it", async () => {
    for (const rel of signingScripts) {
      const bytes = await readFile(join(repoRoot, rel));
      const hasBom = bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf;
      const offenders = nonAsciiLines(bytes);
      expect(
        hasBom || offenders.length === 0,
        `${rel} has non-ASCII bytes without a UTF-8 BOM on line(s) ${offenders.slice(0, 10).join(", ")}`,
      ).toBe(true);
    }
  });

  it("keeps the script tauri.conf.json runs via `powershell` ASCII-only", async () => {
    const conf = JSON.parse(
      await readFile(join(repoRoot, "src-tauri/tauri.conf.json"), "utf8"),
    );
    const hook = conf.bundle.windows.signCommand;
    expect(hook.cmd).toBe("powershell");
    const rel = hook.args.find((a) => a.endsWith(".ps1"));
    expect(rel).toBeTruthy();
    // tauri build runs the hook with cwd = src-tauri/.
    const abs = resolve(repoRoot, "src-tauri", rel);
    expect(abs).toBe(signScript);
    const bytes = await readFile(abs);
    expect(nonAsciiLines(bytes)).toEqual([]);
  });
});

const pwshProbe = spawnSync(
  "pwsh",
  ["-NoProfile", "-Command", "$PSVersionTable.PSVersion.Major"],
  { encoding: "utf8" },
);
const hasPwsh = pwshProbe.status === 0;

const configEnvKeys = [
  "WINDOWS_SIGNING_PROVIDER",
  "WINDOWS_CERTIFICATE",
  "WINDOWS_CERTIFICATE_PASSWORD",
  "WINDOWS_SIGNING_THUMBPRINT",
  "WINDOWS_TIMESTAMP_URL",
  "WINDOWS_SIGNING_FAILURE_MARKER",
];

function runSign(args, env) {
  const cleanEnv = { ...process.env };
  for (const k of configEnvKeys) delete cleanEnv[k];
  return spawnSync("pwsh", ["-NoProfile", "-File", signScript, ...args], {
    encoding: "utf8",
    env: { ...cleanEnv, ...env },
  });
}

async function withMarker(fn) {
  const dir = await mkdtemp(join(tmpdir(), "sign-test-"));
  try {
    return await fn(join(dir, "marker"));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

// Non-signing logic only: signtool and the certificate store are Windows
// specific, so provider dispatch, the migration guard, third-party plugin
// filtering and the failure marker are what is testable off Windows. The
// actual signing path is proven by win-installer-check.yml.
describe.skipIf(!hasPwsh)("sign-windows-authenticode.ps1 dispatch (pwsh)", () => {
  it("is a silent no-op when no provider is configured", () => {
    const r = runSign(["-Path", "does-not-matter.exe"], {});
    expect(r.status).toBe(0);
    expect(r.stdout).toContain("not set - skipping Authenticode signing");
  });

  it.each(["none", "NONE", "  "])("treats provider %j as unsigned", (p) => {
    const r = runSign(["-Path", "x.exe"], { WINDOWS_SIGNING_PROVIDER: p });
    expect(r.status).toBe(0);
  });

  it("refuses to silently downgrade when only the legacy PFX secret is set", () =>
    withMarker(async (marker) => {
      const r = runSign(["-Path", "x.exe"], {
        WINDOWS_CERTIFICATE: "AAAA",
        WINDOWS_SIGNING_FAILURE_MARKER: marker,
      });
      expect(r.status).not.toBe(0);
      expect(r.stdout).toContain(
        "WINDOWS_CERTIFICATE is set but WINDOWS_SIGNING_PROVIDER is not",
      );
      expect(existsSync(marker)).toBe(true);
    }));

  it("fails and writes the failure marker for an unknown provider", () =>
    withMarker(async (marker) => {
      const r = runSign(["-Path", "x.exe"], {
        WINDOWS_SIGNING_PROVIDER: "bogus",
        WINDOWS_SIGNING_FAILURE_MARKER: marker,
      });
      expect(r.status).not.toBe(0);
      expect(r.stdout).toContain("unknown WINDOWS_SIGNING_PROVIDER");
      expect(await readFile(marker, "utf8")).toContain(
        "unknown WINDOWS_SIGNING_PROVIDER",
      );
    }));

  it.each(["esigner", "certum"])("requires a thumbprint for %s", (provider) =>
    withMarker(async (marker) => {
      const r = runSign(["-Path", "x.exe"], {
        WINDOWS_SIGNING_PROVIDER: provider,
        WINDOWS_SIGNING_FAILURE_MARKER: marker,
      });
      expect(r.status).not.toBe(0);
      expect(r.stdout).toContain("WINDOWS_SIGNING_THUMBPRINT is empty");
      expect(existsSync(marker)).toBe(true);
    }),
  );

  it("requires the PFX for local-pfx", () => {
    const r = runSign(["-Path", "x.exe"], {
      WINDOWS_SIGNING_PROVIDER: "local-pfx",
    });
    expect(r.status).not.toBe(0);
    expect(r.stdout).toContain("WINDOWS_CERTIFICATE is empty");
  });

  it("skips third-party NSIS plugin DLLs even with a broken provider", () => {
    const r = runSign(
      ["-Path", "C:\\x\\NSISdl.dll", "/tmp/nsis_tauri_utils.dll", "System.dll"],
      { WINDOWS_SIGNING_PROVIDER: "bogus" },
    );
    expect(r.status).toBe(0);
    expect(r.stdout).toContain("Nothing left to sign");
    expect(r.stdout).not.toContain("unknown WINDOWS_SIGNING_PROVIDER");
  });

  it("does not write a failure marker on the unsigned default path", () =>
    withMarker(async (marker) => {
      const r = runSign(["-Path", "x.exe"], {
        WINDOWS_SIGNING_FAILURE_MARKER: marker,
      });
      expect(r.status).toBe(0);
      expect(existsSync(marker)).toBe(false);
    }));
});
