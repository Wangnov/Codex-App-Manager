import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import {
  CATALOG,
  dirOf,
  I18nProvider,
  LANGS,
  matchTag,
  pickLang,
  useI18n,
  type Lang,
  type TKey,
} from "./i18n";

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

function placeholders(value: string): string[] {
  return Array.from(value.matchAll(/\{(\w+)\}/g), (match) => match[1]).sort();
}

describe("catalog placeholders", () => {
  it("keeps placeholder sets aligned with English in every language", () => {
    for (const key of Object.keys(CATALOG.en) as Array<keyof typeof CATALOG.en>) {
      const expected = placeholders(CATALOG.en[key]);
      for (const lang of LANGS.map((item) => item.code)) {
        expect(placeholders(CATALOG[lang][key]), `${lang}:${key}`).toEqual(expected);
      }
    }
  });
});

const SAFETY_CRITICAL_PREFIXES = [
  "install.partial.",
  "settings.health.",
  "uninstall.status.",
  "uninstall.partial.",
] as const;

const SOURCE_LANGS = ["en", "zh-CN"] as const satisfies readonly Lang[];
const SAFETY_SOURCE_COPY_ALLOWLIST = new Map<string, string>([
  // Map each exception to the exact source key whose normalized copy may match.
  ["zh-TW:settings.health.ok", "zh-CN:settings.health.ok"], // 「正常」 is idiomatic in both Chinese variants.
  ["ja:settings.health.ok", "zh-CN:settings.health.ok"], // 「正常」 is also the native Japanese status label.
]);

function normalizedCopy(value: string): string {
  return value.normalize("NFC").replace(/\s+/g, " ").trim();
}

const safetyCriticalKeys = (Object.keys(CATALOG.en) as TKey[]).filter((key) =>
  SAFETY_CRITICAL_PREFIXES.some((prefix) => key.startsWith(prefix)),
);

describe("safety-critical catalog copy", () => {
  it("does not copy an English or Simplified-Chinese source paragraph into another locale", () => {
    const translatedLangs = LANGS.map((item) => item.code).filter(
      (lang) => !SOURCE_LANGS.includes(lang as (typeof SOURCE_LANGS)[number]),
    );
    const sourceOwnersByCopy = new Map<string, string[]>();
    for (const source of SOURCE_LANGS) {
      for (const key of safetyCriticalKeys) {
        const copy = normalizedCopy(CATALOG[source][key]);
        sourceOwnersByCopy.set(copy, [
          ...(sourceOwnersByCopy.get(copy) ?? []),
          `${source}:${key}`,
        ]);
      }
    }

    const unapprovedCopies: string[] = [];
    const usedAllowlistEntries = new Set<string>();

    for (const lang of translatedLangs) {
      for (const key of safetyCriticalKeys) {
        const candidate = normalizedCopy(CATALOG[lang][key]);
        const copiedFrom = sourceOwnersByCopy.get(candidate);
        const allowlistEntry = `${lang}:${key}`;
        const approvedSourceOwner = SAFETY_SOURCE_COPY_ALLOWLIST.get(allowlistEntry);

        if (copiedFrom != null) {
          if (approvedSourceOwner && copiedFrom.includes(approvedSourceOwner)) {
            usedAllowlistEntries.add(allowlistEntry);
          } else {
            unapprovedCopies.push(`${allowlistEntry} duplicates ${copiedFrom.join(", ")}`);
          }
        }
      }
    }

    expect(unapprovedCopies).toEqual([]);
    expect(
      [...SAFETY_SOURCE_COPY_ALLOWLIST.keys()].filter(
        (entry) => !usedAllowlistEntries.has(entry),
      ),
    ).toEqual([]);
  });

  it("keeps partial-uninstall recovery actions distinct in every language", () => {
    const recoveryKeys = [
      "uninstall.partial.retryCleanup",
      "uninstall.partial.retryProvenance",
      "uninstall.partial.retryPurge",
      "uninstall.partial.retryRecord",
    ] as const satisfies readonly TKey[];

    for (const { code } of LANGS) {
      const labels = recoveryKeys.map((key) => normalizedCopy(CATALOG[code][key]));
      expect(new Set(labels).size, code).toBe(recoveryKeys.length);
      expect(CATALOG[code]["settings.health.resetConfirm.body.settings"], code).not.toBe(
        CATALOG[code]["settings.health.resetConfirm.body.provenance"],
      );
      expect(CATALOG[code]["settings.health.reset"], code).not.toBe(
        CATALOG[code]["settings.health.clearProvenance"],
      );
    }
  });

  it("keeps recovery instructions bound to their rendered action labels", () => {
    for (const { code } of LANGS) {
      expect(placeholders(CATALOG[code]["install.partial.note"]), code).toContain("action");
      expect(placeholders(CATALOG[code]["install.partial.pending"]), code).toContain("action");
      expect(
        placeholders(CATALOG[code]["settings.health.resetConfirm.body.provenance"]),
        code,
      ).toContain("action");
    }
  });

  it("ships native safety-copy sentinels for the required flow locales", () => {
    expect(CATALOG.fr["install.partial.note"]).toContain("enregistrement de gestion");
    expect(CATALOG.ar["settings.health.resetConfirm.body.provenance"]).toContain("سجلات إدارة");
    expect(CATALOG.es["uninstall.partial.retryPurge"]).toContain("datos de usuario");
    expect(CATALOG["zh-TW"]["settings.health.restoreConfirm.body"]).toContain("目前內容會被取代");
  });
});

function DirProbe() {
  const { setLang } = useI18n();
  return (
    <>
      <button onClick={() => setLang("ar")}>Arabic</button>
      <button onClick={() => setLang("en")}>English</button>
    </>
  );
}

describe("I18nProvider", () => {
  it("updates document direction for RTL and LTR languages", async () => {
    const user = userEvent.setup();
    localStorage.setItem("cam.lang", "en");
    render(
      <I18nProvider>
        <DirProbe />
      </I18nProvider>,
    );

    await user.click(screen.getByRole("button", { name: "Arabic" }));
    expect(document.documentElement.dir).toBe("rtl");

    await user.click(screen.getByRole("button", { name: "English" }));
    expect(document.documentElement.dir).toBe("ltr");
  });
});
