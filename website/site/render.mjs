// Renders the static pages (/, /en/, /404.html) from the locale dictionaries
// and the build-time data snapshot. Runs in Node only (Vite plugin + scripts);
// the browser bundle (src/main.ts) just enhances this markup.

import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import zh from "./locales/zh.mjs";
import en from "./locales/en.mjs";
import {
  ORIGIN,
  SKINS_ORIGIN,
  HERO_SKINS,
  FEATURED_SKINS,
  CATEGORY_ORDER,
  REPO,
  MANAGER_FILES,
  CODEX_FILES,
  LINKS,
  COMMANDS,
} from "./config.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.dirname(here);
const readData = () => JSON.parse(readFileSync(path.join(here, "data/site.json"), "utf8"));

export const DICTS = { zh, en };

/* ------------------------------------------------------------------ helpers */

const esc = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const fill = (s, vars) => s.replace(/\{(\w+)\}/g, (_, k) => (k in vars ? vars[k] : `{${k}}`));

const iconCache = new Map();
/** Inline Phosphor icon (regular weight unless stated). */
function icon(name, cls = "i", weight = "regular") {
  const key = `${weight}/${name}`;
  if (!iconCache.has(key)) {
    const file = weight === "regular" ? `${name}.svg` : `${name}-${weight}.svg`;
    const svg = readFileSync(path.join(root, "node_modules/@phosphor-icons/core/assets", weight, file), "utf8");
    iconCache.set(key, svg.trim());
  }
  return iconCache.get(key).replace("<svg ", `<svg class="${cls}" aria-hidden="true" focusable="false" `);
}

/** MiB with Manager-style rounding ("677 MB", "19.6 MB"). */
export function formatBytes(bytes) {
  if (!bytes) return "";
  const mb = bytes / 1024 / 1024;
  return `${mb >= 100 ? Math.round(mb) : mb.toFixed(1)} MB`;
}

export function formatDate(iso, lang) {
  const d = new Date(iso);
  return lang === "zh"
    ? `${d.getUTCFullYear()} 年 ${d.getUTCMonth() + 1} 月 ${d.getUTCDate()} 日`
    : d.toLocaleDateString("en-US", { year: "numeric", month: "short", day: "numeric", timeZone: "UTC" });
}

/** "1.09M" badge text -> "109 万" / "1.09M". */
export function formatDownloads(badge, lang) {
  const m = /^([\d.]+)\s*([kKmM]?)$/.exec(String(badge ?? "").trim());
  if (!m) return badge ?? "";
  const n = parseFloat(m[1]) * ({ k: 1e3, m: 1e6 }[m[2].toLowerCase()] ?? 1);
  if (lang === "zh" && n >= 1e4) return `${Math.floor(n / 1e4)}<span class="stat-unit">万</span>`;
  return esc(badge);
}

const shortHash = (h) => (h ? `${h.slice(0, 10)}…${h.slice(-6)}` : "");

/** "大圣云途 · Journey to the West" -> the half that matches the page language. */
function shortName(name, lang) {
  const parts = name.split(" · ");
  const cjk = /[\u3400-\u9fff]/;
  if (parts.length === 2 && cjk.test(parts[0]) && !cjk.test(parts[1])) return lang === "zh" ? parts[0] : parts[1];
  return name;
}

/** Theme-aware <picture>: dark by default, light via prefers-color-scheme. */
function themedPicture({ dark, light, widths, sizes, alt, width, height, cls = "", eager = false, formats = ["avif", "webp"] }) {
  const srcset = (base, fmt) => widths.map((w) => `/img/${base}-${w}.${fmt} ${w}w`).join(", ");
  const sources = [];
  for (const fmt of formats) {
    sources.push(`<source media="(prefers-color-scheme: light)" type="image/${fmt}" srcset="${srcset(light, fmt)}" sizes="${sizes}">`);
  }
  for (const fmt of formats.slice(0, -1)) {
    sources.push(`<source type="image/${fmt}" srcset="${srcset(dark, fmt)}" sizes="${sizes}">`);
  }
  const last = formats[formats.length - 1];
  const loading = eager ? `fetchpriority="high"` : `loading="lazy"`;
  return `<picture class="${cls}">${sources.join("")}<img src="/img/${dark}-${widths[widths.length - 1]}.${last}" srcset="${srcset(dark, last)}" sizes="${sizes}" alt="${esc(alt)}" width="${width}" height="${height}" decoding="async" ${loading}></picture>`;
}

function copyButton(t, text) {
  return `<button class="copy-btn" type="button" data-copy="${esc(text)}" data-label="${esc(t.download.copy)}" data-done="${esc(t.download.copied)}">${icon("copy")}<span>${esc(t.download.copy)}</span></button>`;
}

function cmd(t, text, label = "") {
  return `<div class="cmd">${label ? `<span class="cmd-label">${label}</span>` : ""}<code>${esc(text)}</code>${copyButton(t, text)}</div>`;
}

function orderedSkins(skins) {
  const byId = new Map(skins.map((s) => [s.id, s]));
  const lead = FEATURED_SKINS.map((id) => byId.get(id)).filter(Boolean);
  const rest = skins.filter((s) => !FEATURED_SKINS.includes(s.id));
  return [...lead, ...rest];
}

/* ------------------------------------------------------- display strings */

/** Every string rendered in the display face (feeds the CJK font subset). */
export function displayStrings(t) {
  return [
    t.brand,
    t.hero.h1,
    t.suite.h2,
    t.suite.manager.role,
    t.suite.mirror.role,
    t.suite.skins.role,
    t.manager.h2,
    ...t.manager.callouts.map((c) => c.title),
    ...t.manager.features.map((f) => f.title),
    t.skins.h2,
    ...t.skins.verbs.map((v) => v.title),
    t.skins.maker.h3,
    t.mirror.h2,
    t.mirror.release.title,
    t.mirror.pipelineTitle,
    ...t.mirror.pipeline.map((p) => p.title),
    ...t.mirror.channels.map((c) => c.title),
    t.trust.h2,
    ...t.trust.points.map((p) => p.title),
    t.trust.nonGoalsTitle,
    t.download.h2,
    t.download.skins.viaManager.title,
    t.download.skins.viaPack.title,
    t.download.skins.viaCli.title,
    t.faq.h2,
    t.notFound.h1,
    t.proof.probe.unit,
    t.proof.skins.unit,
    "万",
  ].filter(Boolean);
}

/* ------------------------------------------------------------------- head */

function head(t, data, page) {
  const url = `${ORIGIN}${t.path}`;
  const title = page === "404" ? t.notFound.title : t.meta.title;
  const redirect =
    t.lang === "zh" && page !== "404"
      ? `<script>(function(){try{if(location.pathname!=="/")return;var s=localStorage.getItem("cam-site-lang");if(s==="zh")return;if(s==="en"||(!/^zh\\b/i.test(navigator.language||"")&&!/bot|crawl|spider|slurp|preview|lighthouse/i.test(navigator.userAgent))){location.replace("/en/"+location.hash)}}catch(e){}})();</script>`
      : "";
  const ld = {
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    name: "Codex App Manager",
    operatingSystem: "macOS, Windows",
    applicationCategory: "DeveloperApplication",
    license: "https://opensource.org/licenses/MIT",
    url: ORIGIN + "/",
    sameAs: [REPO.manager, REPO.mirror, REPO.skins],
    offers: { "@type": "Offer", price: "0", priceCurrency: "USD" },
  };
  return `
    <title>${esc(title)}</title>
    <meta name="description" content="${esc(t.meta.description)}">
    ${page === "404" ? `<meta name="robots" content="noindex">` : `<link rel="canonical" href="${url}">`}
    <link rel="alternate" hreflang="zh-CN" href="${ORIGIN}/">
    <link rel="alternate" hreflang="en" href="${ORIGIN}/en/">
    <link rel="alternate" hreflang="x-default" href="${ORIGIN}/">
    <meta property="og:type" content="website">
    <meta property="og:site_name" content="${esc(t.brand)}">
    <meta property="og:title" content="${esc(t.meta.ogTitle)}">
    <meta property="og:description" content="${esc(t.meta.ogDescription)}">
    <meta property="og:url" content="${url}">
    <meta property="og:image" content="${ORIGIN}/img/${t.meta.ogImage}">
    <meta property="og:image:width" content="1200">
    <meta property="og:image:height" content="630">
    <meta property="og:locale" content="${t.meta.ogLocale}">
    <meta name="twitter:card" content="summary_large_image">
    <meta name="theme-color" content="#090a14" media="(prefers-color-scheme: dark)">
    <meta name="theme-color" content="#f5f6fb" media="(prefers-color-scheme: light)">
    <link rel="icon" type="image/png" href="/img/logo-manager-192.png">
    <link rel="apple-touch-icon" href="/img/logo-manager-192.png">
    <link rel="preload" href="/fonts/mona-sans.woff2" as="font" type="font/woff2" crossorigin>
    ${t.lang === "zh" ? `<link rel="preload" href="/fonts/shs-heavy.woff2" as="font" type="font/woff2" crossorigin>` : ""}
    ${page === "404" ? "" : `<link rel="preload" as="image" type="image/avif" media="(prefers-color-scheme: dark)" imagesrcset="/img/hero-dark-960.avif 960w, /img/hero-dark-1672.avif 1672w" imagesizes="100vw">
    <link rel="preload" as="image" type="image/avif" media="(prefers-color-scheme: light)" imagesrcset="/img/hero-light-960.avif 960w, /img/hero-light-1672.avif 1672w" imagesizes="100vw">`}
    ${redirect}
    <script type="application/ld+json">${JSON.stringify(ld)}</script>`;
}

/* -------------------------------------------------------------------- nav */

function nav(t) {
  const other = t.lang === "zh" ? "en" : "zh";
  const links = [
    ["#manager", t.nav.manager],
    ["#skins", t.nav.skins],
    ["#mirror", t.nav.mirror],
    ["#faq", t.nav.faq],
  ];
  return `
  <a class="skip-link" href="#main">${esc(t.nav.skip)}</a>
  <header class="nav" data-nav>
    <div class="container nav-inner">
      <a class="brand" href="#top"><img src="/img/logo-manager-192.png" alt="" width="28" height="28"><span>${esc(t.brand)}</span></a>
      <nav class="nav-links" aria-label="${esc(t.nav.primaryLabel)}">
        ${links.map(([h, l]) => `<a href="${h}">${esc(l)}</a>`).join("")}
      </nav>
      <div class="nav-actions">
        <a class="icon-btn" href="${REPO.mirror}" aria-label="${esc(t.nav.github)}">${icon("github-logo")}</a>
        <a class="lang-switch" href="${t.nav.langHref}" hreflang="${other === "zh" ? "zh-CN" : "en"}" lang="${other === "zh" ? "zh-CN" : "en"}" aria-label="${esc(t.nav.langAria)}" data-lang-switch="${other}">${icon("translate")}<span>${esc(t.nav.lang)}</span></a>
        <a class="btn btn-primary btn-sm nav-cta" href="#download">${esc(t.nav.download)}</a>
        <button class="icon-btn nav-burger" type="button" aria-expanded="false" aria-controls="mobile-menu" aria-label="${esc(t.nav.menu)}" data-label-open="${esc(t.nav.menu)}" data-label-close="${esc(t.nav.close)}">${icon("list", "i i-open")}${icon("x", "i i-close")}</button>
      </div>
    </div>
    <div class="mobile-menu" id="mobile-menu" hidden>
      <nav class="container" aria-label="${esc(t.nav.primaryLabel)}">
        ${links.map(([h, l]) => `<a href="${h}">${esc(l)}</a>`).join("")}
        <a href="#download">${esc(t.nav.download)}</a>
      </nav>
    </div>
  </header>`;
}

/* ------------------------------------------------------------------- hero */

function hero(t, data) {
  const byId = new Map(data.skins.map((s) => [s.id, s]));
  const frames = HERO_SKINS.map((id, i) => {
    const name = byId.get(id)?.name ?? id;
    const alt = fill(t.hero.codexAlt, { name });
    const attr = (a, v) => (i === 0 ? `${a}="${v}"` : `data-${a}="${v}"`);
    return `<picture class="stage-frame${i === 0 ? " is-active" : ""}" data-skin="${id}">
        <source type="image/avif" ${attr("srcset", `/img/skins/${id}-1280.avif`)}>
        <img ${attr("src", `/img/skins/${id}-1280.webp`)} alt="${esc(alt)}" width="1280" height="800" decoding="async"${i === 0 ? ` fetchpriority="high"` : ""}>
      </picture>`;
  }).join("");
  const dots = HERO_SKINS.map((id, i) => {
    const name = shortName(byId.get(id)?.name ?? id, t.lang);
    return `<button class="skin-dot" type="button" data-skin="${id}" data-name="${esc(name)}" aria-pressed="${i === 0}" aria-label="${esc(name)}"><img src="/img/skins/${id}-dot.webp" alt="" width="48" height="48" loading="lazy"></button>`;
  }).join("");
  const first = shortName(byId.get(HERO_SKINS[0])?.name ?? "", t.lang);
  return `
  <section class="hero" id="top">
    <div class="hero-bg" aria-hidden="true">
      ${themedPicture({ dark: "hero-dark", light: "hero-light", widths: [960, 1672], sizes: "100vw", alt: "", width: 1672, height: 941, eager: true })}
    </div>
    <div class="container hero-grid">
      <div class="hero-copy">
        <a class="status-pill" href="#mirror">
          <span class="live-dot" aria-hidden="true"></span>
          <span>${esc(t.hero.statusPrefix)} <b class="mono" data-live="codex-version">${esc(data.codex.version)}</b></span>
          <time class="status-age" data-live="codex-age" datetime="${data.codex.publishedAt}">${esc(formatDate(data.codex.publishedAt, t.lang))}</time>
        </a>
        <h1>${t.hero.h1.split("<br>").map((l) => `<span class="h1-line">${l}</span>`).join("")}</h1>
        <p class="hero-lead">${esc(t.hero.lead)}</p>
        <div class="hero-ctas">
          <a class="btn btn-primary btn-lg" id="hero-dl" href="#download" data-open-tab="manager">${icon("download-simple")}<span>${esc(t.hero.ctaManager)}</span><span class="btn-hint" data-platform-hint hidden></span></a>
          <a class="btn btn-glass btn-lg" href="#download" data-open-tab="codex">${esc(t.hero.ctaCodex)}</a>
        </div>
      </div>
      <div class="hero-stage">
        <div class="stage-media">
          <div class="stage-window">${frames}</div>
          ${themedPicture({ dark: `manager/compact-dark-${t.lang}`, light: `manager/compact-light-${t.lang}`, widths: [560], sizes: "280px", alt: t.hero.managerAlt, width: 428, height: 668, cls: "stage-manager", formats: ["webp"] })}
        </div>
        <div class="stage-switcher" role="group" aria-label="${esc(t.hero.switcherLabel)}">
          <div class="skin-dots">${dots}</div>
          <p class="stage-caption" aria-live="polite"><span>${esc(t.hero.wearing)}</span> <b data-stage-name>${esc(first)}</b></p>
        </div>
      </div>
    </div>
  </section>`;
}

/* ------------------------------------------------------------------ proof */

function proof(t, data) {
  const stars = Object.values(data.stars).reduce((a, b) => a + b, 0);
  const starsRounded = `${(Math.floor(stars / 100) * 100).toLocaleString("en-US")}+`;
  const stat = (num, label, note) =>
    `<div class="stat" data-reveal><p class="stat-num">${num}</p><p class="stat-label">${esc(label)}</p><p class="stat-note">${esc(note)}</p></div>`;
  return `
  <section class="proof" aria-label="${esc(t.proof.aria)}">
    <div class="container proof-grid">
      ${stat(`<span data-live="downloads">${formatDownloads(data.downloads, t.lang)}</span>`, t.proof.downloads.label, t.proof.downloads.note)}
      ${stat(esc(starsRounded), t.proof.stars.label, t.proof.stars.note)}
      ${stat(`${t.proof.probe.num}<span class="stat-unit">${esc(t.proof.probe.unit)}</span>`, t.proof.probe.label, t.proof.probe.note)}
      ${stat(`${data.skins.length}${t.proof.skins.unit ? `<span class="stat-unit">${esc(t.proof.skins.unit)}</span>` : ""}`, t.proof.skins.label, t.proof.skins.note)}
    </div>
  </section>`;
}

/* ------------------------------------------------------------------ suite */

function suite(t, data) {
  const s = t.suite;
  const byId = new Map(data.skins.map((x) => [x.id, x]));
  const mosaic = ["celestial-court", "shinji-eva01", "journey-to-west", "gundam-rx78"]
    .filter((id) => byId.has(id))
    .map((id) => `<img src="/img/skins/${id}-640.webp" alt="" width="640" height="400" loading="lazy" decoding="async">`)
    .join("");
  const card = (key, cls, logo, repoName, href, visual) => `
      <article class="suite-card ${cls}" data-reveal>
        ${visual}
        <div class="suite-body">
          <p class="suite-repo">${logo}<span class="mono">${esc(repoName)}</span></p>
          <h3>${esc(s[key].role)}</h3>
          <p>${esc(s[key].desc)}</p>
          <a class="arrow-link" href="${href}">${esc(s[key].link)}${icon("arrow-right")}</a>
        </div>
      </article>`;
  return `
  <section class="section suite" id="suite">
    <span class="anchor-alias" id="why" aria-hidden="true"></span>
    <div class="container">
      <header class="section-head" data-reveal>
        <h2>${esc(s.h2)}</h2>
        <p class="lead">${esc(s.lead)}</p>
      </header>
      <div class="suite-grid">
        ${card(
          "manager",
          "card-manager",
          `<img src="/img/logo-manager-192.png" alt="" width="24" height="24">`,
          "Codex App Manager",
          "#manager",
          `<div class="suite-visual" aria-hidden="true"><img class="suite-obj obj-sync" src="/img/obj/sync-640.webp" alt="" width="640" height="640" loading="lazy" decoding="async"></div>`
        )}
        ${card(
          "mirror",
          "card-mirror",
          `<img src="/img/logo-mirror-192.png" alt="" width="24" height="24">`,
          "codex-app-mirror",
          "#mirror",
          `<div class="suite-visual" aria-hidden="true">${themedPicture({ dark: "lake-dark", light: "lake-light", widths: [960], sizes: "(min-width: 900px) 40vw, 100vw", alt: "", width: 960, height: 540, cls: "suite-bg" })}<img class="suite-obj obj-cloud" src="/img/obj/cloud-640.webp" alt="" width="640" height="640" loading="lazy" decoding="async"></div>`
        )}
        ${card(
          "skins",
          "card-skins",
          `<img src="/img/obj/cards-320.webp" alt="" width="24" height="24">`,
          "awesome-codex-skins",
          "#skins",
          `<div class="suite-visual" aria-hidden="true"><div class="suite-mosaic">${mosaic}</div><img class="suite-obj obj-cards" src="/img/obj/cards-640.webp" alt="" width="640" height="640" loading="lazy" decoding="async"></div>`
        )}
      </div>
      <ol class="suite-flow" aria-label="${esc(s.flowLabel)}" data-reveal>
        ${s.flow.map((f, i) => `<li>${i ? icon("arrow-right", "i flow-arrow") : ""}<span>${esc(f)}</span></li>`).join("")}
      </ol>
    </div>
  </section>`;
}

/* ---------------------------------------------------------------- manager */

function manager(t, data) {
  const m = t.manager;
  const pos = [
    { x: 87.6, y: 45.2, side: "right" },
    { x: 87.6, y: 66.8, side: "right" },
    { x: 52.4, y: 74.6, side: "left" },
  ];
  const callouts = m.callouts
    .map((c, i) => `<li class="callout" data-side="${pos[i].side}" style="--x:${pos[i].x}%;--y:${pos[i].y}%"><span class="callout-dot" aria-hidden="true"></span><span class="callout-card"><b>${esc(c.title)}</b><span>${esc(c.body)}</span></span></li>`)
    .join("");
  const icons = ["arrows-clockwise", "arrow-counter-clockwise", "clock-counter-clockwise", "windows-logo", "globe-hemisphere-east"];
  const [delta, ...rest] = m.features;
  return `
  <section class="section manager" id="manager">
    <div class="container">
      <header class="section-head" data-reveal>
        <p class="product-tag"><img src="/img/logo-manager-192.png" alt="" width="32" height="32"><span>Codex App Manager</span><span class="tag-ver mono">v<span data-live="manager-version">${esc(data.manager.version)}</span></span></p>
        <h2>${esc(m.h2)}</h2>
        <p class="lead">${esc(m.lead)}</p>
      </header>
      <figure class="annotated" data-reveal>
        ${themedPicture({ dark: `manager/home-dark-${t.lang}`, light: `manager/home-light-${t.lang}`, widths: [1200, 2256], sizes: "(min-width: 1240px) 1100px, 92vw", alt: m.alt, width: 1128, height: 748, formats: ["webp"] })}
        <ul class="callouts">${callouts}</ul>
      </figure>
      <div class="mgr-bento">
        <article class="mgr-cell cell-delta" data-reveal>
          <div class="delta-figure" aria-hidden="true"><span class="delta-num">96.8<small>%</small></span></div>
          <h3>${esc(delta.title)}</h3>
          <p>${esc(delta.body)}</p>
          <div class="bars" role="img" aria-label="${esc(`${delta.deltaLabel} 19.6 MB, ${delta.fullLabel} 623 MB`)}">
            <div class="bar-row"><span class="bar-label">${esc(delta.deltaLabel)}</span><span class="bar bar-delta" style="--w:3.15%"></span><span class="bar-val mono">19.6 MB</span></div>
            <div class="bar-row"><span class="bar-label">${esc(delta.fullLabel)}</span><span class="bar bar-full" style="--w:100%"></span><span class="bar-val mono">623 MB</span></div>
          </div>
        </article>
        ${rest.map((f, i) => `<article class="mgr-cell" data-reveal><span class="cell-icon">${icon(icons[i + 1])}</span><h3>${esc(f.title)}</h3><p>${esc(f.body)}</p></article>`).join("")}
      </div>
      <ul class="spec-row" data-reveal>${m.specs.map((s) => `<li>${esc(s)}</li>`).join("")}</ul>
      <div class="mgr-cta" data-reveal>
        <a class="btn btn-primary btn-lg" href="#download" data-open-tab="manager">${icon("download-simple")}<span>${esc(m.cta)}</span></a>
        ${cmd(t, COMMANDS.brew)}
      </div>
    </div>
  </section>`;
}

/* ------------------------------------------------------------------ skins */

function skins(t, data) {
  const k = t.skins;
  const list = orderedSkins(data.skins);
  const counts = Object.fromEntries(CATEGORY_ORDER.map((c) => [c, data.skins.filter((s) => s.category === c).length]));
  const filters = [
    `<button type="button" role="radio" aria-checked="true" data-filter="all">${esc(k.all)}<span class="count">${data.skins.length}</span></button>`,
    ...CATEGORY_ORDER.filter((c) => counts[c]).map(
      (c) => `<button type="button" role="radio" aria-checked="false" tabindex="-1" data-filter="${c}">${esc(k.categories[c] ?? c)}<span class="count">${counts[c]}</span></button>`
    ),
  ].join("");
  const cards = list
    .map(
      (s, i) => `<li class="skin-item" data-cat="${s.category}" data-rank="${i}">
        <button class="skin-card" type="button" aria-haspopup="dialog" data-skin="${s.id}" data-name="${esc(s.name)}" data-desc="${esc(s.description)}" data-version="${esc(s.version)}" data-verified="${esc(s.codexVerified ?? "")}" data-appearance="${esc(s.appearance ?? "")}" data-pack="${esc(`${SKINS_ORIGIN}/${s.pack}`)}" data-cat-label="${esc(k.categories[s.category] ?? s.category)}">
          <span class="skin-thumb"><img src="/img/skins/${s.id}-640.webp" alt="${esc(fill(k.cardAlt, { name: s.name }))}" width="640" height="400" loading="lazy" decoding="async"></span>
          <span class="skin-meta"><span class="skin-name">${esc(s.name)}</span><span class="skin-cat">${esc(k.categories[s.category] ?? s.category)}</span></span>
        </button>
      </li>`
    )
    .join("");
  const verbIcons = ["t-shirt", "check-circle", "arrow-counter-clockwise"];
  return `
  <section class="section skins" id="skins">
    <div class="container">
      <header class="section-head" data-reveal>
        <h2>${esc(k.h2)}</h2>
        <p class="lead">${esc(k.lead)}</p>
      </header>
      <div class="filter" role="radiogroup" aria-label="${esc(k.filterLabel)}">${filters}</div>
      <ul class="skins-grid" data-collapsed="true">${cards}</ul>
      <div class="skins-more">
        <button class="btn btn-ghost" type="button" data-skins-toggle aria-expanded="false" data-more="${esc(fill(k.showAll, { n: data.skins.length }))}" data-less="${esc(k.showLess)}">${esc(fill(k.showAll, { n: data.skins.length }))}${icon("caret-down")}</button>
      </div>
      <div class="skins-flow">
        <ol class="verbs">
          ${k.verbs.map((v, i) => `<li data-reveal><span class="verb-icon">${icon(verbIcons[i])}</span><div><h3>${esc(v.title)}</h3><p>${esc(v.body)}</p></div></li>`).join("")}
          <li class="safety" data-reveal>${icon("shield-check")}<p>${esc(k.safety)}</p></li>
        </ol>
        <figure class="store-shot" data-reveal>
          ${themedPicture({ dark: `manager/store-dark-${t.lang}`, light: `manager/store-light-${t.lang}`, widths: [1200, 2256], sizes: "(min-width: 1100px) 640px, 92vw", alt: k.storeAlt, width: 1128, height: 748, formats: ["webp"] })}
        </figure>
      </div>
      <aside class="maker" data-reveal>
        <div class="maker-copy">
          <h3>${esc(k.maker.h3)}</h3>
          <p>${esc(k.maker.body)}</p>
          ${cmd(t, COMMANDS.skill)}
          <p class="maker-links"><a class="arrow-link" href="${LINKS.spec}">${esc(k.maker.spec)}${icon("arrow-up-right")}</a><a class="arrow-link" href="${REPO.skins}${k.maker.submitHash}">${esc(k.maker.submit)}${icon("arrow-up-right")}</a></p>
        </div>
        <img class="maker-obj" src="/img/obj/cards-640.webp" alt="" width="640" height="640" loading="lazy" decoding="async">
      </aside>
      <p class="fineprint">${esc(k.disclaimer)}</p>
    </div>
  </section>`;
}

function skinDialog(t) {
  const d = t.skins.dialog;
  return `
  <dialog class="skin-dialog" id="skin-dialog" aria-labelledby="sd-name" data-appearance-dual="${esc(d.appearance.dual)}" data-appearance-dark="${esc(d.appearance.dark)}" data-appearance-light="${esc(d.appearance.light)}">
    <div class="sd-media"><img id="sd-img" alt="" width="1280" height="800"></div>
    <div class="sd-body">
      <p class="sd-cat" id="sd-cat"></p>
      <h3 id="sd-name"></h3>
      <p class="sd-desc" id="sd-desc"></p>
      <dl class="sd-meta">
        <div><dt>${esc(d.version)}</dt><dd class="mono" id="sd-version"></dd></div>
        <div><dt>${esc(d.verified)}</dt><dd class="mono" id="sd-verified"></dd></div>
        <div><dt>${esc(d.appearanceLabel)}</dt><dd id="sd-appearance"></dd></div>
      </dl>
      <div class="sd-actions">
        <a class="btn btn-primary" href="#download" data-open-tab="skins" data-close-dialog>${icon("download-simple")}<span>${esc(d.install)}</span></a>
        <a class="btn btn-ghost" id="sd-pack" href="${SKINS_ORIGIN}">${icon("file-archive")}<span>${esc(d.pack)}</span></a>
      </div>
    </div>
    <button class="icon-btn sd-close" type="button" aria-label="${esc(d.close)}" data-close-dialog>${icon("x")}</button>
    <button class="icon-btn sd-nav sd-prev" type="button" aria-label="${esc(d.prev)}" data-step="-1">${icon("caret-left")}</button>
    <button class="icon-btn sd-nav sd-next" type="button" aria-label="${esc(d.next)}" data-step="1">${icon("caret-right")}</button>
  </dialog>`;
}

/* ----------------------------------------------------------------- mirror */

function mirror(t, data) {
  const r = t.mirror;
  const files = CODEX_FILES.map((f) => {
    const info = data.codex.files[f.key];
    return `<li class="rc-file">
      <span class="rc-plat">${icon(f.icon)}<span>${esc(r.platforms[f.key])}</span></span>
      <span class="rc-size mono" data-live="size-${f.key}">${esc(formatBytes(info?.bytes))}</span>
      <span class="rc-hash mono" data-live="hash-${f.key}" title="SHA-256">${esc(shortHash(info?.sha256))}</span>
      <a class="rc-dl" href="${f.href}" aria-label="${esc(`${r.release.download} ${r.platforms[f.key]}`)}">${icon("download-simple")}</a>
    </li>`;
  }).join("");
  const stageIcons = ["package", "timer", "seal-check", "hard-drives", "globe-hemisphere-east"];
  const channelLinks = [LINKS.latestRelease, LINKS.beta, LINKS.linux];
  const channelIcons = ["check-circle", "flask", "linux-logo"];
  return `
  <section class="section mirror" id="mirror">
    <span class="anchor-alias" id="pipeline" aria-hidden="true"></span>
    <div class="mirror-banner">
      ${themedPicture({ dark: "lake-dark", light: "lake-light", widths: [960, 1672], sizes: "100vw", alt: "", width: 1672, height: 941, cls: "banner-bg" })}
      <div class="container banner-inner">
        <h2 class="mirror-title"><span class="title-text">${esc(r.h2)}</span><span class="title-reflection" aria-hidden="true">${esc(r.h2)}</span></h2>
      </div>
    </div>
    <div class="container">
      <p class="lead mirror-lead" data-reveal>${esc(r.lead)}</p>
      <div class="mirror-grid">
        <article class="release-card" data-reveal>
          <header class="rc-head">
            <p class="rc-title"><span class="live-dot" aria-hidden="true"></span>${esc(r.release.title)}</p>
            <p class="rc-version">Codex <span class="mono" data-live="codex-version">${esc(data.codex.version)}</span></p>
            <p class="rc-date">${esc(r.release.published)} <time data-live="codex-date" datetime="${data.codex.publishedAt}">${esc(formatDate(data.codex.publishedAt, t.lang))}</time></p>
          </header>
          <ul class="rc-files">${files}</ul>
          <footer class="rc-links">
            <a href="${LINKS.checksums}">${icon("fingerprint")}${esc(r.release.sums)}</a>
            <a href="${LINKS.manifest}">${icon("brackets-curly")}${esc(r.release.manifest)}</a>
            <a href="${LINKS.history}">${icon("clock-counter-clockwise")}${esc(r.release.history)}</a>
          </footer>
        </article>
        <div class="pipeline" data-reveal>
          <h3>${esc(r.pipelineTitle)}</h3>
          <ol class="pipe">
            ${r.pipeline.map((p, i) => `<li><span class="pipe-node">${icon(stageIcons[i])}</span><div><h4>${esc(p.title)}</h4><p>${esc(p.body)}</p></div></li>`).join("")}
          </ol>
          <img class="pipeline-obj" src="/img/obj/globe-640.webp" alt="" width="640" height="640" loading="lazy" decoding="async">
        </div>
      </div>
      <ul class="channels" data-reveal>
        ${r.channels.map((c, i) => `<li><span class="ch-icon">${icon(channelIcons[i])}</span><h3>${esc(c.title)}</h3><p>${esc(c.body)}</p><a class="arrow-link" href="${channelLinks[i]}">${esc(r.channelLink)}${icon("arrow-up-right")}</a></li>`).join("")}
      </ul>
    </div>
  </section>`;
}

/* ------------------------------------------------------------------ trust */

function trust(t, data) {
  const tr = t.trust;
  const term = tr.terminal;
  const mac = data.codex.files["mac-arm64"]?.sha256 ?? "";
  const win = (data.codex.files["win-x64"]?.sha256 ?? "").toUpperCase();
  const pointIcons = ["seal-check", "fingerprint", "info", "github-logo"];
  return `
  <section class="section trust" id="trust">
    <span class="anchor-alias" id="scope" aria-hidden="true"></span>
    <div class="container trust-grid">
      <div class="trust-copy">
        <header class="section-head" data-reveal>
          <h2>${esc(tr.h2)}</h2>
          <p class="lead">${esc(tr.lead)}</p>
        </header>
        <ul class="trust-points">
          ${tr.points.map((p, i) => `<li data-reveal><span class="tp-icon">${icon(pointIcons[i])}</span><h3>${esc(p.title)}</h3><p>${esc(p.body)}</p></li>`).join("")}
        </ul>
        <div class="nongoals" data-reveal>
          <h3>${esc(tr.nonGoalsTitle)}</h3>
          <ul>${tr.nonGoals.map((g) => `<li>${icon("x-circle")}<span>${esc(g)}</span></li>`).join("")}</ul>
        </div>
      </div>
      <div class="terminal-wrap" data-reveal>
        <img class="trust-obj" src="/img/obj/shield-640.webp" alt="" width="640" height="640" loading="lazy" decoding="async">
        <div class="terminal">
          <div class="term-bar">
            <span class="term-lights" aria-hidden="true"><i></i><i></i><i></i></span>
            <span class="term-title">${esc(term.title)}</span>
            <div class="term-tabs" role="tablist" aria-label="${esc(term.title)}">
              <button type="button" role="tab" aria-selected="true" aria-controls="term-mac" id="term-tab-mac" data-os="mac">${esc(term.mac)}</button>
              <button type="button" role="tab" aria-selected="false" aria-controls="term-win" id="term-tab-win" data-os="win" tabindex="-1">${esc(term.win)}</button>
            </div>
          </div>
          <pre class="term-body" id="term-mac" role="tabpanel" aria-labelledby="term-tab-mac"><span class="tc">${esc(term.cDownload)}</span>
<span class="tp">$</span> curl -L -o Codex-mac-arm64.dmg \\
    ${ORIGIN}/latest/mac-arm64
<span class="tc">${esc(term.cHash)}</span>
<span class="tp">$</span> shasum -a 256 Codex-mac-arm64.dmg
<span class="th" data-live="sha-mac-arm64">${esc(mac)}</span>
  Codex-mac-arm64.dmg
<span class="tok">✓ ${esc(term.match)}</span></pre>
          <pre class="term-body" id="term-win" role="tabpanel" aria-labelledby="term-tab-win"><span class="tc">${esc(term.cDownloadWin)}</span>
<span class="tp">PS&gt;</span> Invoke-WebRequest ${ORIGIN}/latest/win-x64 \`
      -OutFile Codex-Windows-x64.msix
<span class="tc">${esc(term.cHashWin)}</span>
<span class="tp">PS&gt;</span> (Get-FileHash .\\Codex-Windows-x64.msix -Algorithm SHA256).Hash
<span class="th" data-live="sha-win-x64">${esc(win)}</span>
<span class="tok">✓ ${esc(term.match)}</span></pre>
        </div>
      </div>
    </div>
  </section>`;
}

/* --------------------------------------------------------------- download */

function download(t, data) {
  const d = t.download;
  const m = t.mirror;
  const fileRow = (f, extra = "") => `<li class="file-row" data-platform="${f.key}">
      <span class="file-plat">${icon(f.icon)}<span>${esc(m.platforms[f.key])}</span></span>
      <span class="file-meta"><span class="file-name mono">${esc(f.file)}</span>${extra}<span class="file-flag">${icon("check")}${esc(d.forYou)}</span></span>
      <a class="btn btn-sm btn-quiet" href="${f.href}">${icon("download-simple")}<span>${esc(t.nav.download)}</span></a>
    </li>`;
  const chips = [
    [LINKS.checksums, "fingerprint", d.codex.checksums],
    [LINKS.manifest, "brackets-curly", d.codex.manifest],
    [LINKS.history, "clock-counter-clockwise", d.codex.history],
    [LINKS.store, "storefront", d.codex.store],
    [LINKS.linux, "linux-logo", d.codex.linux],
    [LINKS.beta, "flask", d.codex.beta],
  ];
  const tab = (key, ic, selected, badge = "") =>
    `<button type="button" role="tab" id="tab-${key}" aria-controls="panel-${key}" aria-selected="${selected}" ${selected ? "" : `tabindex="-1"`} data-tab="${key}">${ic}<span>${esc(d.tabs[key])}</span>${badge}</button>`;
  return `
  <section class="section download" id="download">
    <div class="container">
      <header class="section-head" data-reveal>
        <h2>${esc(d.h2)}</h2>
        <p class="lead">${esc(d.lead)}</p>
      </header>
      <img class="download-obj" src="/img/obj/cloud-640.webp" alt="" width="640" height="640" loading="lazy" decoding="async">
      <div class="dl-shell" data-reveal>
        <div class="dl-tabs" role="tablist" aria-label="${esc(d.tabsLabel)}">
          ${tab("manager", `<img src="/img/logo-manager-192.png" alt="" width="22" height="22">`, true, `<span class="badge">${esc(d.recommended)}</span>`)}
          ${tab("codex", `<img src="/img/logo-mirror-192.png" alt="" width="22" height="22">`, false)}
          ${tab("skins", `<img src="/img/obj/cards-320.webp" alt="" width="22" height="22">`, false)}
        </div>
        <div class="dl-panel" role="tabpanel" id="panel-manager" aria-labelledby="tab-manager">
          <h3 class="panel-title">${esc(d.tabs.manager)}</h3>
          <p class="panel-intro">${esc(d.manager.intro)}</p>
          <div class="panel-grid">
            <div class="pm">
              ${cmd(t, COMMANDS.brew, `${icon("apple-logo")}${esc(d.manager.brew)}`)}
              ${cmd(t, COMMANDS.winget, `${icon("windows-logo")}${esc(d.manager.winget)}`)}
            </div>
            <ul class="file-list">${MANAGER_FILES.map((f) => fileRow(f)).join("")}</ul>
          </div>
          <p class="panel-note">${fill(d.manager.note, { version: `<span class="mono" data-live="manager-version">${esc(data.manager.version)}</span>` })}</p>
        </div>
        <div class="dl-panel" role="tabpanel" id="panel-codex" aria-labelledby="tab-codex">
          <h3 class="panel-title">${esc(d.tabs.codex)}</h3>
          <p class="panel-intro">${esc(d.codex.intro)}</p>
          <ul class="file-list">${CODEX_FILES.map((f) => fileRow(f, `<span class="file-size mono" data-live="size-${f.key}">${esc(formatBytes(data.codex.files[f.key]?.bytes))}</span>`)).join("")}</ul>
          <p class="link-chips">${chips.map(([h, ic, l]) => `<a href="${h}">${icon(ic)}${esc(l)}</a>`).join("")}</p>
        </div>
        <div class="dl-panel" role="tabpanel" id="panel-skins" aria-labelledby="tab-skins">
          <h3 class="panel-title">${esc(d.tabs.skins)}</h3>
          <p class="panel-intro">${esc(d.skins.intro)}</p>
          <ul class="routes">
            <li><span class="route-icon">${icon("storefront")}</span><div><h3>${esc(d.skins.viaManager.title)}</h3><p>${esc(d.skins.viaManager.body)}</p></div><a class="btn btn-sm btn-quiet" href="#download" data-open-tab="manager">${esc(d.tabs.manager)}</a></li>
            <li><span class="route-icon">${icon("file-archive")}</span><div><h3>${esc(d.skins.viaPack.title)}</h3><p>${esc(d.skins.viaPack.body)}</p></div><a class="btn btn-sm btn-quiet" href="${LINKS.skinPacks}">${esc(d.skins.viaPack.link)}${icon("arrow-up-right")}</a></li>
            <li class="route-cli"><span class="route-icon">${icon("terminal-window")}</span><div><h3>${esc(d.skins.viaCli.title)}</h3><p>${esc(d.skins.viaCli.body)}</p><pre class="code-block">${COMMANDS.studio.map((l) => `<span class="tp">$</span> ${esc(l)}`).join("\n")}</pre></div></li>
          </ul>
        </div>
      </div>
    </div>
  </section>`;
}

/* -------------------------------------------------------------------- faq */

function faq(t) {
  const f = t.faq;
  return `
  <section class="section faq" id="faq">
    <div class="container faq-grid">
      <header class="faq-head" data-reveal>
        <h2>${esc(f.h2)}</h2>
        <p class="lead">${esc(f.lead)}</p>
        <a class="btn btn-ghost" href="${LINKS.issues}">${icon("chat-circle")}<span>${esc(f.ask)}</span></a>
      </header>
      <div class="faq-list">
        ${f.items.map((it) => `<details name="faq" data-reveal><summary><span>${esc(it.q)}</span>${icon("plus", "i faq-plus")}</summary><div class="faq-a"><p>${it.a}</p></div></details>`).join("")}
      </div>
    </div>
  </section>`;
}

/* ----------------------------------------------------------------- footer */

function footer(t) {
  const f = t.footer;
  const col = (title, links) =>
    `<div><h3>${esc(title)}</h3><ul>${links.map(([h, l]) => `<li><a href="${h}">${esc(l)}</a></li>`).join("")}</ul></div>`;
  return `
  <footer class="footer">
    <div class="footer-sky" aria-hidden="true">
      ${themedPicture({ dark: "hero-dark", light: "hero-light", widths: [960, 1672], sizes: "100vw", alt: "", width: 1672, height: 941 })}
    </div>
    <div class="container footer-inner">
      <div class="footer-top">
        <div class="footer-brand">
          <a class="brand" href="#top"><img src="/img/logo-manager-192.png" alt="" width="32" height="32"><span>${esc(t.brand)}</span></a>
          <p>${esc(f.tagline)}</p>
          <a class="sponsor" href="${LINKS.duckcoding}">
            <img src="/img/sponsor-duckcoding.webp" alt="DuckCoding" width="40" height="40" loading="lazy">
            <span><b>${esc(f.sponsorLabel)} · DuckCoding</b><span>${esc(f.sponsorText)}</span></span>
          </a>
        </div>
        <nav class="footer-cols" aria-label="${esc(f.navLabel)}">
          ${col(f.cols.projects, [
            [REPO.manager, "Codex App Manager"],
            [REPO.mirror, "codex-app-mirror"],
            [REPO.skins, "awesome-codex-skins"],
          ])}
          ${col(f.cols.docs, [
            [LINKS.signing, f.links.signing],
            [LINKS.privacy, f.links.privacy],
            [LINKS.spec, f.links.spec],
            [LINKS.registry, f.links.registry],
          ])}
          ${col(f.cols.community, [
            [LINKS.linuxdo, "LINUX DO"],
            [LINKS.issues, f.links.issues],
            [LINKS.history, f.links.releases],
          ])}
        </nav>
      </div>
      <p class="footer-thanks">${esc(f.thanks)}</p>
      <div class="footer-bottom">
        <p>${esc(f.disclaimer)}</p>
        <p><a href="${REPO.mirror}/blob/main/LICENSE">${esc(f.license)}</a><a href="#top" class="to-top">${esc(f.top)}${icon("arrow-up-right")}</a></p>
      </div>
    </div>
  </footer>`;
}

/* ------------------------------------------------------------------ pages */

function notFound(t) {
  const n = t.notFound;
  return `
  <main id="main" class="nf">
    <div class="hero-bg" aria-hidden="true">
      ${themedPicture({ dark: "hero-dark", light: "hero-light", widths: [960, 1672], sizes: "100vw", alt: "", width: 1672, height: 941, eager: true })}
    </div>
    <div class="container nf-inner">
      <a class="brand" href="/"><img src="/img/logo-manager-192.png" alt="" width="32" height="32"><span>${esc(t.brand)}</span></a>
      <p class="nf-code mono">404</p>
      <h1>${esc(n.h1)}</h1>
      <p class="lead">${esc(n.body)}</p>
      <p class="nf-actions"><a class="btn btn-primary btn-lg" href="/">${esc(n.home)}</a><a class="btn btn-glass btn-lg" href="/en/">English</a></p>
    </div>
  </main>`;
}

/**
 * @param {string} shell  HTML shell with <!--app-head--> and <!--app-body-->
 * @param {"zh"|"en"|"404"} page
 */
export function renderPage(shell, page) {
  const data = readData();
  const t = page === "en" ? en : zh;
  const body =
    page === "404"
      ? notFound(t)
      : `${nav(t)}
  <main id="main">
    ${hero(t, data)}
    ${proof(t, data)}
    ${suite(t, data)}
    ${manager(t, data)}
    ${skins(t, data)}
    ${mirror(t, data)}
    ${trust(t, data)}
    ${download(t, data)}
    ${faq(t)}
  </main>
  ${footer(t)}
  ${skinDialog(t)}`;
  return shell
    .replace("%LANG%", t.htmlLang)
    .replace("<!--app-head-->", head(t, data, page))
    .replace("<!--app-body-->", body);
}
