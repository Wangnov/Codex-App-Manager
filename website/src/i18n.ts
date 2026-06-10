import zh from "./locales/zh";
import en from "./locales/en";

export type Lang = "zh" | "en";

const dicts: Record<Lang, unknown> = { zh, en };
const STORAGE_KEY = "cam-site-lang";

export function initialLang(): Lang {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === "zh" || stored === "en") return stored;
  } catch {
    /* storage unavailable */
  }
  return navigator.language?.toLowerCase().startsWith("zh") ? "zh" : "en";
}

export function t(lang: Lang, path: string): unknown {
  return path
    .split(".")
    .reduce<any>((node, key) => (node == null ? node : node[key]), dicts[lang]);
}

function setMeta(selector: string, value: string) {
  document.querySelector(selector)?.setAttribute("content", value);
}

export function applyLang(lang: Lang) {
  document.documentElement.lang = lang === "zh" ? "zh-CN" : "en";
  document.body.dataset.lang = lang;
  try {
    localStorage.setItem(STORAGE_KEY, lang);
  } catch {
    /* storage unavailable */
  }

  document.querySelectorAll<HTMLElement>("[data-i18n]").forEach((el) => {
    const value = t(lang, el.dataset.i18n!);
    if (typeof value === "string") el.textContent = value;
  });
  document.querySelectorAll<HTMLElement>("[data-i18n-aria]").forEach((el) => {
    const value = t(lang, el.dataset.i18nAria!);
    if (typeof value === "string") el.setAttribute("aria-label", value);
  });
  document.querySelectorAll<HTMLImageElement>("[data-i18n-alt]").forEach((el) => {
    const value = t(lang, el.dataset.i18nAlt!);
    if (typeof value === "string") el.alt = value;
  });

  document.title = t(lang, "meta.title") as string;
  setMeta('meta[name="description"]', t(lang, "meta.description") as string);
  setMeta('meta[property="og:title"]', t(lang, "meta.ogTitle") as string);
  setMeta('meta[property="og:description"]', t(lang, "meta.ogDescription") as string);
  setMeta('meta[property="og:locale"]', lang === "zh" ? "zh_CN" : "en_US");
  setMeta(
    'meta[property="og:locale:alternate"]',
    lang === "zh" ? "en_US" : "zh_CN"
  );
}
