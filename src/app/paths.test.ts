import { describe, expect, it } from "vitest";

import { codexHomeDisplay } from "./paths";

describe("codexHomeDisplay", () => {
  it("uses a Windows-specific symbolic path", () => {
    expect(codexHomeDisplay("windows")).toBe("%USERPROFILE%\\.codex");
  });

  it("uses ~/.codex for macOS and other platforms", () => {
    expect(codexHomeDisplay("macos")).toBe("~/.codex");
    expect(codexHomeDisplay("other")).toBe("~/.codex");
  });
});
