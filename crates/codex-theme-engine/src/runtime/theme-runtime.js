// Renderer-side runtime. Injected via Runtime.evaluate — must be idempotent,
// re-entrant and fully reversible. Placeholders are substituted by payload.mjs.
//
// Flicker discipline: ensure() runs on every DOM mutation, so EVERY write in
// here must be guarded by a value comparison — writing the same value to
// style/class/attributes still dirties style state in Chromium and causes
// visible repaint flashes (e.g. whenever a dropdown portal mounts).
((cssText, themeConfig, chromeHtml) => {
  const STATE_KEY = "__CODEX_THEME_STUDIO__";
  const DISABLED_KEY = "__CODEX_THEME_STUDIO_DISABLED__";
  const STYLE_ID = "cts-style";
  const CHROME_ID = "cts-chrome";
  const STAGE_ID = "cts-stage";
  const INTRO_ID = "cts-intro";
  const ROOT_CLASS = "codex-theme-studio";
  const THEME_ATTR = "data-cts-theme";
  const SHELL_ATTR = "data-cts-shell";
  const VERSION = __CTS_VERSION_JSON__;
  const STAMP = __CTS_STAMP_JSON__;
  const THEME = themeConfig && typeof themeConfig === "object" ? themeConfig : {};

  window[DISABLED_KEY] = false;

  // Tear down any previous install (idempotent re-entry, incl. theme switch).
  const previous = window[STATE_KEY];
  if (previous?.observer) previous.observer.disconnect();
  if (previous?.timer) clearInterval(previous.timer);
  if (previous?.clock) clearInterval(previous.clock);
  if (previous?.scheduler?.timeout) clearTimeout(previous.scheduler.timeout);
  if (previous?.resizeHandler) window.removeEventListener("resize", previous.resizeHandler);
  if (previous?.mediaHandler && previous?.mediaQuery) {
    try { previous.mediaQuery.removeEventListener("change", previous.mediaHandler); } catch {}
  }
  if (previous?.appliedVars) {
    for (const name of previous.appliedVars) document.documentElement?.style.removeProperty(name);
  }

  // Split the chrome fragment into its layers: "overlay" floats above the UI
  // (fixed, z31), "stage" is scenery mounted inside main UNDER the content.
  // Fragments without layer markers keep the legacy all-overlay behaviour.
  const layers = (() => {
    const tpl = document.createElement("template");
    tpl.innerHTML = chromeHtml || "";
    const overlay = tpl.content.querySelector('[data-cts-layer="overlay"]');
    const stage = tpl.content.querySelector('[data-cts-layer="stage"]');
    return {
      overlayHtml: overlay ? overlay.innerHTML : (stage ? "" : (chromeHtml || "")),
      stageHtml: stage ? stage.innerHTML : "",
    };
  })();

  const appliedVars = [];
  const setVar = (name, value) => {
    const root = document.documentElement;
    if (root.style.getPropertyValue(name) !== value) root.style.setProperty(name, value);
    if (!appliedVars.includes(name)) appliedVars.push(name);
  };

  const setAttr = (node, name, value) => {
    if (node.getAttribute(name) !== value) node.setAttribute(name, value);
  };

  const setClass = (node, name, on) => {
    if (node.classList.contains(name) !== on) node.classList.toggle(name, on);
  };

  const detectShellMode = () => {
    const root = document.documentElement;
    const cls = `${root.className || ""} ${document.body?.className || ""}`.toLowerCase();
    if (/\b(dark|theme-dark|appearance-dark)\b/.test(cls)) return "dark";
    if (/\b(light|theme-light|appearance-light)\b/.test(cls)) return "light";
    const dataTheme = (
      root.getAttribute("data-theme") || root.getAttribute("data-appearance") ||
      root.getAttribute("data-color-mode") || document.body?.getAttribute("data-theme") || ""
    ).toLowerCase();
    if (dataTheme.includes("dark")) return "dark";
    if (dataTheme.includes("light")) return "light";
    try {
      if (window.matchMedia("(prefers-color-scheme: dark)").matches) return "dark";
    } catch {}
    return "light";
  };

  // Sticky route detection: only flip home-state on positive signals, so
  // transient DOM (dropdown portals, dialogs) never toggles theme classes.
  const findHome = (sticky) => {
    const indicator = document.querySelector('[data-testid="home-icon"]');
    if (indicator) return indicator.closest('[role="main"]');
    const bySuggestions = [...document.querySelectorAll('[role="main"]')]
      .find((candidate) => candidate.querySelector('.group\\/home-suggestions'));
    if (bySuggestions) return bySuggestions;
    if (sticky?.isConnected) return sticky; // keep last known while it lives
    return null;
  };

  const chromeRectCache = { left: NaN, top: NaN, width: NaN, height: NaN };

  // Semantic icon annotation: CSS cannot match by text, so tag well-known
  // controls with data-cts-icon and let theme CSS attach bitmap icons.
  // Idempotent — tagged nodes are skipped, and the attribute is not in the
  // observer's attributeFilter, so tagging never re-triggers ensure().
  const SIDEBAR_ICONS = [
    { icon: "new-task", texts: ["新建任务", "New task"] },
    { icon: "scheduled", texts: ["已安排", "Scheduled"] },
    { icon: "plugins", texts: ["插件", "Plugins"] },
    { icon: "sites", texts: ["站点", "Sites"] },
    { icon: "pull-request", texts: ["拉取请求", "Pull request"] },
    { icon: "chat", texts: ["聊天", "Chat"] },
  ];
  const CARD_ICONS = ["explore", "build", "review", "fix"];

  // The glyph attribute lands on the FIRST svg inside the control, so sibling
  // svgs (dropdown chevrons etc.) keep their native artwork.
  const tagGlyph = (container, icon) => {
    if (!container || container.dataset.ctsIcon) return;
    const svg = container.querySelector("svg");
    if (!svg) return;
    container.dataset.ctsIcon = icon;
    svg.dataset.ctsGlyph = icon;
  };

  const annotateIcons = () => {
    const aside = document.querySelector(".app-shell-left-panel");
    if (aside) {
      for (const button of aside.querySelectorAll("button:not([data-cts-icon])")) {
        const text = button.textContent || "";
        const rule = SIDEBAR_ICONS.find((entry) => entry.texts.some((t) => text.includes(t)));
        if (rule) tagGlyph(button, rule.icon);
      }
      const search = aside.querySelector('[aria-label="搜索"]:not([data-cts-icon]), [aria-label="Search"]:not([data-cts-icon])');
      if (search) tagGlyph(search, "search");
      // Workspace title → tokusatsu logo. The title text is split across
      // child spans ("ChatGPT" + "工作"), so match on the whole button and
      // re-evaluate every pass (the same button swaps text on switch).
      for (const button of aside.querySelectorAll("button")) {
        const text = button.textContent.replace(/\s+/g, " ").trim();
        const isCodex = text === "Codex";
        const isWork = /^ChatGPT ?(工作|Work)$/i.test(text);
        if (!isCodex && !isWork) {
          if (button.dataset.ctsLogo) delete button.dataset.ctsLogo;
          continue;
        }
        const want = isCodex ? "codex" : "chatgpt-work";
        if (button.dataset.ctsLogo !== want) button.dataset.ctsLogo = want;
      }
    }
    const composer = document.querySelector(".composer-surface-chrome");
    if (composer) {
      for (const button of composer.querySelectorAll("button:not([data-cts-icon])")) {
        const aria = button.getAttribute("aria-label") || "";
        const text = button.textContent || "";
        if (aria.includes("添加文件") || aria.toLowerCase().includes("add file")) tagGlyph(button, "attach");
        else if (aria.includes("听写") || /dictat/i.test(aria)) tagGlyph(button, "mic");
        else if (button.querySelector("svg") && /sol|spark|codex|gpt/i.test(text)) tagGlyph(button, "model");
      }
    }
    document.querySelectorAll('.cts-home .group\\/home-suggestions .grid > div').forEach((cell, index) => {
      const button = cell.querySelector("button:not([data-cts-icon])");
      if (button && CARD_ICONS[index]) tagGlyph(button, CARD_ICONS[index]);
    });
  };

  const ensure = () => {
    if (window[DISABLED_KEY]) return;
    const root = document.documentElement;
    if (!root || !document.body) return;
    const state = window[STATE_KEY];

    setClass(root, ROOT_CLASS, true);
    setAttr(root, THEME_ATTR, THEME.id || "custom");
    setAttr(root, SHELL_ATTR, detectShellMode());

    for (const [key, value] of Object.entries(THEME.colors || {})) setVar(`--cts-color-${key}`, value);
    for (const [key, value] of Object.entries(THEME.strings || {})) setVar(`--cts-str-${key}`, JSON.stringify(String(value)));

    let style = document.getElementById(STYLE_ID);
    if (!style) {
      style = document.createElement("style");
      style.id = STYLE_ID;
      (document.head || root).appendChild(style);
    }
    if (style.dataset.ctsStamp !== STAMP) {
      style.textContent = cssText;
      style.dataset.ctsStamp = STAMP;
    }

    const shellMain = document.querySelector("main.main-surface") || document.querySelector("main");
    const home = findHome(state?.homeSticky);
    if (state) state.homeSticky = home;
    for (const candidate of document.querySelectorAll('[role="main"].cts-home')) {
      if (candidate !== home) candidate.classList.remove("cts-home");
    }
    if (home) setClass(home, "cts-home", true);
    if (shellMain) setClass(shellMain, "cts-home-shell", Boolean(home));

    annotateIcons();

    const fillTexts = (rootNode) => {
      for (const node of rootNode.querySelectorAll("[data-cts-text]")) {
        const key = node.getAttribute("data-cts-text");
        const value = (THEME.strings || {})[key];
        if (typeof value === "string" && node.textContent !== value) node.textContent = value;
      }
    };

    // Stage layer: theme scenery INSIDE main, painted UNDER the app content
    // (main > * are lifted to z-index 1 by the theme CSS). Never overlays
    // dialogs, popovers or panels.
    if (layers.stageHtml && shellMain) {
      let stage = document.getElementById(STAGE_ID);
      if (!stage || stage.parentElement !== shellMain) {
        stage?.remove();
        stage = document.createElement("div");
        stage.id = STAGE_ID;
        stage.setAttribute("aria-hidden", "true");
        stage.style.position = "absolute";
        stage.style.inset = "0";
        stage.style.zIndex = "0";
        stage.style.pointerEvents = "none";
        stage.style.overflow = "hidden";
        shellMain.prepend(stage);
      }
      if (stage.dataset.ctsStamp !== STAMP) {
        stage.innerHTML = layers.stageHtml;
        stage.dataset.ctsStamp = STAMP;
      }
      fillTexts(stage);
      setClass(stage, "cts-home-shell", Boolean(home));
    } else if (!layers.stageHtml) {
      document.getElementById(STAGE_ID)?.remove();
    }

    // Decorative chrome overlay — strictly non-interactive. Full-screen
    // routes (Settings) unmount the shell: hide the chrome entirely there.
    const existingChrome = document.getElementById(CHROME_ID);
    if (existingChrome) {
      const wantVisible = Boolean(layers.overlayHtml && shellMain);
      const visibleNow = existingChrome.style.display !== "none";
      if (visibleNow !== wantVisible) existingChrome.style.display = wantVisible ? "" : "none";
    }
    if (layers.overlayHtml && shellMain) {
      let chrome = document.getElementById(CHROME_ID);
      if (!chrome || chrome.parentElement !== document.body) {
        chrome?.remove();
        chrome = document.createElement("div");
        chrome.id = CHROME_ID;
        chrome.setAttribute("aria-hidden", "true");
        chrome.style.position = "fixed";
        chrome.style.pointerEvents = "none";
        chrome.style.overflow = "hidden";
        chrome.style.zIndex = "31";
        document.body.appendChild(chrome);
      }
      if (chrome.dataset.ctsStamp !== STAMP) {
        chrome.innerHTML = layers.overlayHtml;
        chrome.dataset.ctsStamp = STAMP;
      }
      fillTexts(chrome);
      const box = shellMain.getBoundingClientRect();
      const next = {
        left: Math.round(box.left), top: Math.round(box.top),
        width: Math.round(box.width), height: Math.round(box.height),
      };
      if (next.left !== chromeRectCache.left || next.top !== chromeRectCache.top ||
          next.width !== chromeRectCache.width || next.height !== chromeRectCache.height) {
        Object.assign(chromeRectCache, next);
        chrome.style.left = `${next.left}px`;
        chrome.style.top = `${next.top}px`;
        chrome.style.width = `${next.width}px`;
        chrome.style.height = `${next.height}px`;
      }
      setClass(chrome, "cts-home-shell", Boolean(home));
      setAttr(chrome, SHELL_ATTR, root.getAttribute(SHELL_ATTR) || "light");
    } else if (!layers.overlayHtml) {
      document.getElementById(CHROME_ID)?.remove();
    }
  };

  const cleanup = () => {
    window[DISABLED_KEY] = true;
    const root = document.documentElement;
    root?.classList.remove(ROOT_CLASS);
    root?.removeAttribute(THEME_ATTR);
    root?.removeAttribute(SHELL_ATTR);
    const state = window[STATE_KEY];
    for (const name of state?.appliedVars ?? appliedVars) root?.style.removeProperty(name);
    document.querySelectorAll(".cts-home").forEach((node) => node.classList.remove("cts-home"));
    document.querySelectorAll(".cts-home-shell").forEach((node) => node.classList.remove("cts-home-shell"));
    document.querySelectorAll("[data-cts-glyph]").forEach((node) => node.removeAttribute("data-cts-glyph"));
    document.querySelectorAll("[data-cts-icon]").forEach((node) => node.removeAttribute("data-cts-icon"));
    document.querySelectorAll("[data-cts-logo]").forEach((node) => node.removeAttribute("data-cts-logo"));
    document.getElementById(STYLE_ID)?.remove();
    document.getElementById(CHROME_ID)?.remove();
    document.getElementById(STAGE_ID)?.remove();
    document.getElementById(INTRO_ID)?.remove();
    state?.observer?.disconnect();
    if (state?.timer) clearInterval(state.timer);
    if (state?.clock) clearInterval(state.clock);
    if (state?.scheduler?.timeout) clearTimeout(state.scheduler.timeout);
    if (state?.resizeHandler) window.removeEventListener("resize", state.resizeHandler);
    if (state?.mediaHandler && state?.mediaQuery) {
      try { state.mediaQuery.removeEventListener("change", state.mediaHandler); } catch {}
    }
    delete window[STATE_KEY];
    return true;
  };

  const scheduler = { timeout: null };
  const scheduleEnsure = () => {
    if (scheduler.timeout) clearTimeout(scheduler.timeout);
    scheduler.timeout = setTimeout(() => {
      scheduler.timeout = null;
      ensure();
    }, 180);
  };

  // Ignore mutations we caused ourselves (chrome text/position, clock ticks,
  // root inline vars) — they must never re-trigger ensure().
  const chromeNode = () => document.getElementById(CHROME_ID);
  const observer = new MutationObserver((mutations) => {
    const chrome = chromeNode();
    for (const mutation of mutations) {
      const target = mutation.target;
      if (chrome && (target === chrome || chrome.contains(target))) continue;
      if (target === document.documentElement && mutation.type === "attributes" && mutation.attributeName === "style") continue;
      scheduleEnsure();
      return;
    }
  });
  observer.observe(document.documentElement, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ["class", "data-theme", "data-appearance", "data-color-mode"],
  });
  const timer = setInterval(ensure, 4000);
  const resizeHandler = scheduleEnsure;
  window.addEventListener("resize", resizeHandler, { passive: true });

  // Live tactical clock — writes only textContent inside #cts-chrome, which
  // the observer filter above ignores.
  const clock = setInterval(() => {
    const node = document.querySelector(`#${CHROME_ID} [data-cts-clock]`);
    if (!node) return;
    const now = new Date();
    const two = (n) => String(n).padStart(2, "0");
    const text = `${two(now.getHours())}:${two(now.getMinutes())}:${two(now.getSeconds())}`;
    if (node.textContent !== text) node.textContent = text;
  }, 1000);

  let mediaQuery = null;
  let mediaHandler = null;
  try {
    mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    mediaHandler = () => scheduleEnsure();
    mediaQuery.addEventListener("change", mediaHandler);
  } catch {}

  window[STATE_KEY] = {
    ensure, cleanup, observer, timer, clock, scheduler, resizeHandler,
    mediaQuery, mediaHandler, appliedVars,
    homeSticky: null,
    stamp: STAMP,
    version: VERSION,
    themeId: THEME.id || "custom",
  };
  ensure();

  // Rise! — transformation intro, played once per fresh theme load (not on
  // idempotent re-ensures). Skips quietly when the punch art is absent or
  // the user prefers reduced motion.
  const playIntro = () => {
    try {
      if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
      if (document.getElementById(INTRO_ID) || !document.body) return;
      // Theme-agnostic convention: themes register their intro art as the
      // asset key "intro"; --cts-asset-tiga-punch is the legacy fallback.
      const styles = getComputedStyle(document.documentElement);
      const art = styles.getPropertyValue("--cts-asset-intro") || styles.getPropertyValue("--cts-asset-tiga-punch");
      if (!art || !art.trim()) return;
      const intro = document.createElement("div");
      intro.id = INTRO_ID;
      intro.setAttribute("aria-hidden", "true");
      intro.innerHTML = '<i class="cts-intro-rays"></i><b class="cts-intro-figure"></b><u class="cts-intro-flash"></u>';
      document.body.appendChild(intro);
      setTimeout(() => intro.remove(), 2500);
    } catch { /* cosmetic only */ }
  };
  if (previous?.stamp !== STAMP) playIntro();

  return { installed: true, version: VERSION, themeId: THEME.id || "custom" };
})(__CTS_CSS_JSON__, __CTS_THEME_JSON__, __CTS_CHROME_JSON__)
