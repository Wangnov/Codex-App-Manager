import { describe, expect, it } from "vitest";

import { dirOf, LANGS, matchTag, pickLang } from "./i18n";

describe("matchTag", () => {
  it("maps traditional-Chinese regions and scripts to zh-TW", () => {
    expect(matchTag("zh-TW")).toBe("zh-TW");
    expect(matchTag("zh-Hant-HK")).toBe("zh-TW");
    expect(matchTag("zh-MO")).toBe("zh-TW");
    expect(matchTag("zh-Hant")).toBe("zh-TW");
  });

  it("maps the remaining Chinese tags to zh-CN", () => {
    expect(matchTag("zh")).toBe("zh-CN");
    expect(matchTag("zh-CN")).toBe("zh-CN");
    expect(matchTag("zh-Hans-SG")).toBe("zh-CN");
  });

  it("falls back from a region tag to its supported primary language", () => {
    expect(matchTag("pt-PT")).toBe("pt-BR"); // only Brazilian Portuguese ships
    expect(matchTag("pt-BR")).toBe("pt-BR"); // exact match
    expect(matchTag("ar-AE")).toBe("ar");
    expect(matchTag("de-AT")).toBe("de");
    expect(matchTag("fr-CA")).toBe("fr");
  });

  it("is case-insensitive on the primary subtag", () => {
    expect(matchTag("EN")).toBe("en");
    expect(matchTag("JA-JP")).toBe("ja");
  });

  it("returns null for unsupported or empty tags", () => {
    expect(matchTag("hi-IN")).toBeNull();
    expect(matchTag("xx")).toBeNull();
    expect(matchTag("")).toBeNull();
  });
});

describe("pickLang", () => {
  it("honors a valid saved choice over the system preferences", () => {
    expect(pickLang("zh-CN", ["fr-FR", "en-US"])).toBe("zh-CN");
  });

  it("ignores an invalid saved value and walks the preferences", () => {
    expect(pickLang("klingon", ["ja-JP", "en"])).toBe("ja");
  });

  it("takes the first supported preference", () => {
    expect(pickLang(null, ["hi-IN", "fr-FR", "de-DE"])).toBe("fr");
  });

  it("falls back to English when nothing is supported", () => {
    expect(pickLang(null, ["hi-IN", "th-TH"])).toBe("en");
    expect(pickLang(null, [])).toBe("en");
  });
});

describe("dirOf", () => {
  it("reports Arabic as RTL and everything else as LTR", () => {
    expect(dirOf("ar")).toBe("rtl");
    expect(dirOf("en")).toBe("ltr");
    expect(dirOf("zh-TW")).toBe("ltr");
  });

  it("keeps exactly one RTL language in the catalogue", () => {
    expect(LANGS.filter((l) => l.dir === "rtl").map((l) => l.code)).toEqual(["ar"]);
  });
});
