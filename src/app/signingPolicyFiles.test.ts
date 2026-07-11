import { describe, expect, it } from "vitest";

import releaseWorkflow from "../../.github/workflows/release.yml?raw";
import readme from "../../README.md?raw";
import codeSigningPolicy from "../../docs/code-signing-policy.md?raw";
import privacyPolicy from "../../docs/privacy.md?raw";
import fallbackRelease from "../../docs/releases/FALLBACK.md?raw";
import releaseTemplate from "../../docs/releases/TEMPLATE.md?raw";
import readinessGuard from "../../scripts/assert-signpath-foundation-ready.ps1?raw";
import website from "../../website/index.html?raw";
import websiteEnglish from "../../website/src/locales/en.ts?raw";
import websiteChinese from "../../website/src/locales/zh.ts?raw";

describe("public SignPath Foundation disclosures", () => {
  it("states the pending status, attribution, members, and privacy behavior", () => {
    expect(codeSigningPolicy).toContain(
      "Free code signing provided by [SignPath.io](https://signpath.io/), certificate by [SignPath Foundation](https://signpath.org/)",
    );
    expect(codeSigningPolicy).toMatch(/The application\s+has not yet been approved/);
    expect(codeSigningPolicy).toContain("[@Wangnov](https://github.com/Wangnov)");
    expect(codeSigningPolicy).toContain("manual approval");
    expect(codeSigningPolicy).toContain("[Privacy policy](./privacy.md)");

    expect(privacyPolicy).toContain("about 1.5 seconds after startup");
    expect(privacyPolicy).toContain("about every six hours");
    expect(privacyPolicy).toContain("Users can independently disable startup and");
    expect(privacyPolicy).toContain("includes no telemetry");
    expect(privacyPolicy).toContain("GitHub General Privacy Statement");
    expect(privacyPolicy).toContain("Cloudflare Privacy Policy");
    expect(privacyPolicy).toContain("OpenAI Privacy Policy");
    expect(privacyPolicy).toContain("Microsoft Privacy Statement");
  });

  it("links the stable policies from download, footer, README, and release copy", () => {
    for (const content of [website, readme, releaseTemplate, fallbackRelease]) {
      expect(content).toContain("docs/code-signing-policy.md");
      expect(content).toContain("docs/privacy.md");
    }

    for (const locale of [websiteChinese, websiteEnglish]) {
      expect(locale).toContain("signingPolicy");
      expect(locale).toContain("privacyPolicy");
    }

    expect(releaseTemplate).not.toContain("均带 Authenticode 发行者签名");
    expect(fallbackRelease).not.toContain("carry an Authenticode publisher signature");
    expect(website).toContain("Free code signing provided by");
    expect(website).toContain("https://signpath.io/");
    expect(website).toContain("https://signpath.org/");
  });
});

describe("Windows release signing readiness", () => {
  it("fails closed without accepting the retired PFX assumptions", () => {
    expect(releaseWorkflow).toContain(
      "Assert SignPath Foundation readiness (fail closed)",
    );
    expect(releaseWorkflow).toContain("assert-signpath-foundation-ready.ps1");
    expect(releaseWorkflow).toContain('ExpectedSubject "SignPath Foundation"');
    expect(releaseWorkflow).not.toContain("WINDOWS_CERTIFICATE");
    expect(releaseWorkflow).not.toContain("prepare-windows-authenticode.ps1");

    expect(readinessGuard).toContain("intentionally has no success path");
    expect(readinessGuard).toContain("throw \"[$Stage] $message\"");
    expect(readinessGuard).not.toMatch(/SIGNPATH_.*READY/);
  });
});
