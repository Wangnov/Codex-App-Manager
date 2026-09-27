import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/sections.css";

// Progressive enhancement only: every section is complete static HTML.
const root = document.documentElement;
root.classList.add("js");

const lang: "zh" | "en" = root.lang.startsWith("zh") ? "zh" : "en";
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;
const SKINS_ORIGIN = "https://skins.agentsmirror.com";

const $ = <T extends Element = HTMLElement>(sel: string, scope: ParentNode = document) =>
  scope.querySelector<T>(sel);
const $$ = <T extends Element = HTMLElement>(sel: string, scope: ParentNode = document) =>
  Array.from(scope.querySelectorAll<T>(sel));

/* --------------------------------------------------------- language choice */

for (const a of $$<HTMLAnchorElement>("[data-lang-switch]")) {
  a.addEventListener("click", () => {
    try {
      localStorage.setItem("cas-lang", a.dataset.langSwitch ?? "");
    } catch {
      /* storage unavailable: the switch still navigates */
    }
  });
}

/* --------------------------------------------------------------------- nav */

const nav = $("[data-nav]");
if (nav) {
  const sentinel = document.createElement("div");
  sentinel.setAttribute("aria-hidden", "true");
  sentinel.style.cssText = "position:absolute;top:0;left:0;width:1px;height:24px;pointer-events:none";
  document.body.prepend(sentinel);
  new IntersectionObserver(([entry]) => nav.classList.toggle("is-scrolled", !entry.isIntersecting)).observe(sentinel);

  const burger = $<HTMLButtonElement>(".nav-burger", nav);
  const menu = $("#mobile-menu");
  const setMenu = (open: boolean) => {
    nav.classList.toggle("is-open", open);
    if (menu) menu.hidden = !open;
    burger?.setAttribute("aria-expanded", String(open));
    burger?.setAttribute("aria-label", (open ? burger.dataset.labelClose : burger.dataset.labelOpen) ?? "");
  };
  burger?.addEventListener("click", () => setMenu(!nav.classList.contains("is-open")));
  for (const a of $$("a", menu ?? nav)) a.addEventListener("click", () => setMenu(false));
  addEventListener("keydown", (e) => {
    if (e.key === "Escape" && nav.classList.contains("is-open")) {
      setMenu(false);
      burger?.focus();
    }
  });
}

/* --------------------------------------------------------- reveal on scroll */

(() => {
  const els = $$("[data-reveal]");
  const groups = new Map<Element, number>();
  for (const el of els) {
    const parent = el.parentElement ?? document.body;
    const i = groups.get(parent) ?? 0;
    groups.set(parent, i + 1);
    el.style.setProperty("--stagger", String(Math.min(i, 6)));
  }
  if (!("IntersectionObserver" in window) || reducedMotion) {
    els.forEach((el) => el.classList.add("is-in"));
    return;
  }
  const io = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        e.target.classList.add("is-in");
        io.unobserve(e.target);
      }
    },
    { rootMargin: "0px 0px -6% 0px", threshold: 0.08 }
  );
  els.forEach((el) => io.observe(el));
})();

/* --------------------------------------------------------------- live data */

type FileInfo = { bytes: number | null; sha256: string | null } | null;
interface Status {
  codex?: { version: string | null; publishedAt: string | null; files: Record<string, FileInfo> };
  manager?: { version: string | null; publishedAt: string | null };
}

const setLive = (key: string, value: string, html = false) => {
  for (const el of $$(`[data-live="${key}"]`)) {
    if (html) el.innerHTML = value;
    else el.textContent = value;
  }
};

function formatBytes(bytes: number | null | undefined) {
  if (!bytes) return "";
  const mb = bytes / 1024 / 1024;
  return `${mb >= 100 ? Math.round(mb) : mb.toFixed(1)} MB`;
}

function formatDate(iso: string) {
  const d = new Date(iso);
  return lang === "zh"
    ? `${d.getFullYear()} 年 ${d.getMonth() + 1} 月 ${d.getDate()} 日`
    : d.toLocaleDateString("en-US", { year: "numeric", month: "short", day: "numeric" });
}

function relativeTime(iso: string) {
  const diff = (new Date(iso).getTime() - Date.now()) / 1000;
  const rtf = new Intl.RelativeTimeFormat(lang === "zh" ? "zh-CN" : "en", { numeric: "auto" });
  const abs = Math.abs(diff);
  if (abs < 3600) return rtf.format(Math.round(diff / 60), "minute");
  if (abs < 86400) return rtf.format(Math.round(diff / 3600), "hour");
  if (abs < 86400 * 30) return rtf.format(Math.round(diff / 86400), "day");
  return formatDate(iso);
}

function applyCodexTimes(iso: string | null | undefined) {
  if (!iso || Number.isNaN(Date.parse(iso))) return;
  for (const el of $$<HTMLTimeElement>('[data-live="codex-age"]')) {
    el.dateTime = iso;
    el.textContent = relativeTime(iso);
    el.title = formatDate(iso);
  }
  for (const el of $$<HTMLTimeElement>('[data-live="codex-date"]')) {
    el.dateTime = iso;
    el.textContent = formatDate(iso);
  }
}

function applyStatus(s: Status) {
  const c = s.codex;
  if (c?.version) setLive("codex-version", c.version);
  if (c?.publishedAt) applyCodexTimes(c.publishedAt);
  for (const [key, info] of Object.entries(c?.files ?? {})) {
    if (!info) continue;
    if (info.bytes) setLive(`size-${key}`, formatBytes(info.bytes));
    if (info.sha256) {
      setLive(`hash-${key}`, `${info.sha256.slice(0, 10)}…${info.sha256.slice(-6)}`);
      setLive(`sha-${key}`, key.startsWith("win") ? info.sha256.toUpperCase() : info.sha256);
    }
  }
  if (s.manager?.version) setLive("manager-version", s.manager.version);
}

function formatDownloads(badge: string) {
  const m = /^([\d.]+)\s*([kKmM]?)$/.exec(badge.trim());
  if (!m) return null;
  const n = parseFloat(m[1]) * ({ k: 1e3, m: 1e6 } as Record<string, number>)[m[2].toLowerCase()] || parseFloat(m[1]);
  if (lang === "zh" && n >= 1e4) return `${Math.floor(n / 1e4)}<span class="stat-unit">万</span>`;
  return badge.replace(/[<>&"]/g, "");
}

async function getJSON<T>(url: string, timeoutMs = 6000): Promise<T> {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), timeoutMs);
  try {
    const res = await fetch(url, { signal: ctrl.signal, headers: { accept: "application/json" } });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return (await res.json()) as T;
  } finally {
    clearTimeout(timer);
  }
}

// Baked values render first; relative time is always computed client-side.
applyCodexTimes($<HTMLTimeElement>('[data-live="codex-age"]')?.dateTime);
if ($('[data-live="codex-version"]')) {
  getJSON<Status>("/api/status.json")
    .then(applyStatus)
    .catch(() => {
      /* keep the build-time snapshot */
    });
  getJSON<{ message?: string }>("/stats/downloads.json")
    .then((b) => {
      const html = b.message ? formatDownloads(b.message) : null;
      if (html) setLive("downloads", html, true);
    })
    .catch(() => {
      /* keep the build-time snapshot */
    });
}

/* -------------------------------------------------------------- platforms */

type Platform = "mac-arm64" | "mac-intel" | "win-x64" | "win-arm64";

async function detectPlatform(): Promise<Platform | null> {
  const ua = navigator.userAgent;
  const isAppleMobile = /iPhone|iPad|iPod/i.test(ua) || (/Macintosh/i.test(ua) && navigator.maxTouchPoints > 1);
  if (isAppleMobile || /Android|CrOS|Linux/i.test(ua)) return null;
  let arch: string | undefined;
  const uad = (navigator as Navigator & {
    userAgentData?: { getHighEntropyValues(h: string[]): Promise<{ architecture?: string }> };
  }).userAgentData;
  if (uad?.getHighEntropyValues) {
    try {
      arch = (await uad.getHighEntropyValues(["architecture"])).architecture;
    } catch {
      /* not exposed */
    }
  }
  if (/Windows/i.test(ua)) return arch === "arm" ? "win-arm64" : "win-x64";
  if (!/Macintosh|Mac OS X/i.test(ua)) return null;
  if (arch === "arm") return "mac-arm64";
  if (arch === "x86") return "mac-intel";
  // No client hints (Safari, Firefox): only trust an explicit GPU name.
  // Safari reports a generic "Apple GPU" on every Mac, so that stays unknown.
  try {
    const gl = document.createElement("canvas").getContext("webgl");
    const dbg = gl?.getExtension("WEBGL_debug_renderer_info");
    const renderer = dbg && gl ? String(gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL)) : "";
    if (/(intel|amd|nvidia|radeon)/i.test(renderer)) return "mac-intel";
    if (/apple m\d|apple silicon/i.test(renderer)) return "mac-arm64";
  } catch {
    /* no WebGL */
  }
  return null;
}

const PLATFORM_HINT: Record<Platform, string> = {
  "mac-arm64": "Apple Silicon",
  "mac-intel": "Intel Mac",
  "win-x64": "Windows",
  "win-arm64": "Windows ARM64",
};

void detectPlatform().then((platform) => {
  if (!platform) return;
  for (const row of $$(`.file-row[data-platform="${platform}"]`)) row.classList.add("is-recommended");
  const managerRow = $<HTMLAnchorElement>(`#panel-manager .file-row[data-platform="${platform}"] a`);
  const cta = $<HTMLAnchorElement>("#hero-dl");
  const hint = $("[data-platform-hint]");
  if (cta && managerRow) {
    cta.href = managerRow.href;
    if (hint) {
      hint.textContent = PLATFORM_HINT[platform];
      hint.hidden = false;
    }
  }
  if (platform.startsWith("win")) selectTerminal("win");
});

/* ------------------------------------------------------------------- tabs */

function wireTabs(buttons: HTMLButtonElement[], onSelect: (btn: HTMLButtonElement) => void) {
  const select = (btn: HTMLButtonElement, focus = false) => {
    for (const b of buttons) {
      const on = b === btn;
      b.setAttribute("aria-selected", String(on));
      b.tabIndex = on ? 0 : -1;
      const panel = document.getElementById(b.getAttribute("aria-controls") ?? "");
      if (panel) panel.hidden = !on;
    }
    if (focus) btn.focus();
    onSelect(btn);
  };
  for (const b of buttons) {
    b.addEventListener("click", () => select(b));
    b.addEventListener("keydown", (e) => {
      const i = buttons.indexOf(b);
      const next =
        e.key === "ArrowRight" ? buttons[(i + 1) % buttons.length]
        : e.key === "ArrowLeft" ? buttons[(i - 1 + buttons.length) % buttons.length]
        : e.key === "Home" ? buttons[0]
        : e.key === "End" ? buttons[buttons.length - 1]
        : null;
      if (next) {
        e.preventDefault();
        select(next, true);
      }
    });
  }
  return select;
}

const dlTabs = $$<HTMLButtonElement>(".dl-tabs [role=tab]");
const selectDl = wireTabs(dlTabs, () => {});
const openTab = (key: string) => {
  const btn = dlTabs.find((b) => b.dataset.tab === key);
  if (btn) selectDl(btn);
};
for (const a of $$<HTMLAnchorElement>("[data-open-tab]")) {
  a.addEventListener("click", () => openTab(a.dataset.openTab ?? "manager"));
}

const termTabs = $$<HTMLButtonElement>(".term-tabs [role=tab]");
const selectTerm = wireTabs(termTabs, () => {});
function selectTerminal(os: "mac" | "win") {
  const btn = termTabs.find((b) => b.dataset.os === os);
  if (btn) selectTerm(btn);
}

/* ------------------------------------------------------------------- copy */

async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.cssText = "position:fixed;opacity:0;pointer-events:none";
    document.body.append(ta);
    ta.select();
    const ok = document.execCommand("copy");
    ta.remove();
    return ok;
  }
}

for (const btn of $$<HTMLButtonElement>(".copy-btn")) {
  const label = $("span", btn);
  let timer = 0;
  btn.addEventListener("click", async () => {
    if (!(await copyText(btn.dataset.copy ?? ""))) return;
    btn.classList.add("is-done");
    if (label) label.textContent = btn.dataset.done ?? "";
    clearTimeout(timer);
    timer = window.setTimeout(() => {
      btn.classList.remove("is-done");
      if (label) label.textContent = btn.dataset.label ?? "";
    }, 1600);
  });
}

/* ------------------------------------------------------ hero skin rotation */

(() => {
  const stage = $(".hero-stage");
  if (!stage) return;
  const frames = $$(".stage-frame", stage);
  const dots = $$<HTMLButtonElement>(".skin-dot", stage);
  const nameEl = $("[data-stage-name]", stage);
  if (frames.length < 2) return;

  const hydrate = (frame: HTMLElement) => {
    for (const el of $$<HTMLSourceElement | HTMLImageElement>("[data-srcset], [data-src]", frame)) {
      if (el.dataset.srcset) {
        el.setAttribute("srcset", el.dataset.srcset);
        delete el.dataset.srcset;
      }
      if (el instanceof HTMLImageElement && el.dataset.src) {
        el.src = el.dataset.src;
        delete el.dataset.src;
      }
    }
  };
  const ready = (frame: HTMLElement) =>
    new Promise<void>((resolve) => {
      const img = $<HTMLImageElement>("img", frame);
      if (!img || (img.complete && img.naturalWidth)) return resolve();
      img.addEventListener("load", () => resolve(), { once: true });
      img.addEventListener("error", () => resolve(), { once: true });
      setTimeout(resolve, 2500);
    });

  let index = 0;
  let timer = 0;
  let hovering = false;
  let visible = true;

  const show = async (i: number) => {
    const next = (i + frames.length) % frames.length;
    hydrate(frames[next]);
    await ready(frames[next]);
    index = next;
    frames.forEach((f, j) => f.classList.toggle("is-active", j === index));
    dots.forEach((d, j) => d.setAttribute("aria-pressed", String(j === index)));
    if (nameEl) nameEl.textContent = dots[index]?.dataset.name ?? "";
    hydrate(frames[(index + 1) % frames.length]);
  };

  const schedule = () => {
    clearTimeout(timer);
    if (reducedMotion || hovering || !visible || document.hidden) return;
    timer = window.setTimeout(async () => {
      await show(index + 1);
      schedule();
    }, 4800);
  };

  dots.forEach((d, i) =>
    d.addEventListener("click", async () => {
      await show(i);
      schedule();
    })
  );
  stage.addEventListener("pointerenter", () => {
    hovering = true;
    schedule();
  });
  stage.addEventListener("pointerleave", () => {
    hovering = false;
    schedule();
  });
  stage.addEventListener("focusin", () => {
    hovering = true;
    schedule();
  });
  stage.addEventListener("focusout", () => {
    hovering = false;
    schedule();
  });
  document.addEventListener("visibilitychange", schedule);
  new IntersectionObserver(([e]) => {
    visible = e.isIntersecting;
    schedule();
  }).observe(stage);

  const warm = () => frames.forEach(hydrate);
  if (document.readyState === "complete") setTimeout(warm, 800);
  else addEventListener("load", () => setTimeout(warm, 800), { once: true });
  schedule();
})();

/* ------------------------------------------------------------ skin gallery */

(() => {
  const grid = $(".skins-grid");
  if (!grid) return;
  const items = $$<HTMLLIElement>(".skin-item", grid);
  const radios = $$<HTMLButtonElement>(".filter [role=radio]");
  const toggle = $<HTMLButtonElement>("[data-skins-toggle]");
  const LIMIT = 12;
  let filter = "all";
  let expanded = false;

  const apply = () => {
    let shown = 0;
    for (const li of items) {
      const match = filter === "all" || li.dataset.cat === filter;
      li.hidden = !match;
      if (match) {
        li.classList.toggle("beyond", filter === "all" && shown >= LIMIT);
        shown++;
      }
    }
    grid.dataset.collapsed = String(!expanded);
    if (toggle) {
      toggle.hidden = filter !== "all";
      toggle.setAttribute("aria-expanded", String(expanded));
      const label = expanded ? toggle.dataset.less : toggle.dataset.more;
      const text = toggle.firstChild;
      if (text && text.nodeType === Node.TEXT_NODE) text.textContent = label ?? "";
    }
  };

  const choose = (btn: HTMLButtonElement, focus = false) => {
    filter = btn.dataset.filter ?? "all";
    for (const r of radios) {
      const on = r === btn;
      r.setAttribute("aria-checked", String(on));
      r.tabIndex = on ? 0 : -1;
    }
    if (focus) btn.focus();
    apply();
  };

  radios.forEach((r, i) => {
    r.addEventListener("click", () => choose(r));
    r.addEventListener("keydown", (e) => {
      const d = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
      if (!d) return;
      e.preventDefault();
      choose(radios[(i + d + radios.length) % radios.length], true);
    });
  });

  toggle?.addEventListener("click", () => {
    expanded = !expanded;
    apply();
    if (!expanded) grid.scrollIntoView({ behavior: reducedMotion ? "auto" : "smooth", block: "start" });
  });
  apply();

  /* ---- detail dialog ---- */
  const dialog = $<HTMLDialogElement>("#skin-dialog");
  if (!dialog || typeof dialog.showModal !== "function") return;
  const img = $<HTMLImageElement>("#sd-img", dialog)!;
  const fields = {
    name: $("#sd-name", dialog)!,
    desc: $("#sd-desc", dialog)!,
    cat: $("#sd-cat", dialog)!,
    version: $("#sd-version", dialog)!,
    verified: $("#sd-verified", dialog)!,
    appearance: $("#sd-appearance", dialog)!,
  };
  const pack = $<HTMLAnchorElement>("#sd-pack", dialog)!;
  let list: HTMLButtonElement[] = [];
  let pos = 0;

  let pending: HTMLImageElement | null = null;
  const render = () => {
    const card = list[pos];
    if (!card) return;
    const d = card.dataset;
    const thumb = $<HTMLImageElement>("img", card);
    // Show the already-cached thumbnail at once, then swap in the full-size
    // capture from the skin CDN when it arrives.
    img.src = thumb?.currentSrc || thumb?.src || "";
    img.alt = thumb?.alt ?? "";
    const full = new Image();
    pending = full;
    full.decoding = "async";
    full.onload = () => {
      if (pending === full) img.src = full.src;
    };
    full.src = `${SKINS_ORIGIN}/previews/${d.skin}.webp?v=${encodeURIComponent(d.version ?? "")}`;
    fields.name.textContent = d.name ?? "";
    fields.desc.textContent = d.desc ?? "";
    fields.cat.textContent = d.catLabel ?? "";
    fields.version.textContent = d.version ? `v${d.version}` : "";
    fields.verified.textContent = d.verified ?? "";
    const ap = d.appearance ?? "";
    fields.appearance.textContent =
      (ap === "dual" ? dialog.dataset.appearanceDual : ap === "dark" ? dialog.dataset.appearanceDark : ap === "light" ? dialog.dataset.appearanceLight : "") ?? "";
    pack.href = d.pack ?? SKINS_ORIGIN;
  };

  const step = (dir: number) => {
    if (!list.length) return;
    pos = (pos + dir + list.length) % list.length;
    render();
  };

  grid.addEventListener("click", (e) => {
    const card = (e.target as Element).closest<HTMLButtonElement>(".skin-card");
    if (!card) return;
    list = $$<HTMLButtonElement>(".skin-card", grid).filter((c) => c.offsetParent !== null);
    pos = Math.max(0, list.indexOf(card));
    render();
    dialog.showModal();
  });
  for (const b of $$<HTMLButtonElement>("[data-step]", dialog)) {
    b.addEventListener("click", () => step(Number(b.dataset.step)));
  }
  dialog.addEventListener("keydown", (e) => {
    if (e.key === "ArrowRight") step(1);
    if (e.key === "ArrowLeft") step(-1);
  });
  for (const el of $$("[data-close-dialog]", dialog)) el.addEventListener("click", () => dialog.close());
  dialog.addEventListener("click", (e) => {
    if (e.target === dialog) dialog.close();
  });
})();
