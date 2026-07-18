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
  const WINDOWS_MENU_CLASS = "cts-windows-menu-bar";
  const WINDOWS_MENU_REGION_ATTR = "data-cts-menu-region";
  const RUNTIME_CSS = `
html.codex-theme-studio .cts-windows-menu-bar {
  position: absolute !important;
  inset: 0 0 auto 0 !important;
  height: var(--cts-windows-menu-height, 36px) !important;
}
html.codex-theme-studio .cts-windows-menu-bar + * > aside.app-shell-left-panel {
  padding-top: calc(var(--cts-windows-menu-height, 36px) + var(--cts-windows-sidebar-padding-top, 0px)) !important;
}
html.codex-theme-studio .cts-windows-menu-bar + * > main.main-surface {
  padding-top: calc(var(--cts-windows-menu-height, 36px) + var(--cts-windows-main-padding-top, 0px)) !important;
}
html.codex-theme-studio .cts-windows-menu-bar [data-cts-menu-region="sidebar"] {
  color: var(--cts-windows-sidebar-foreground) !important;
  -webkit-text-fill-color: var(--cts-windows-sidebar-foreground) !important;
}
html.codex-theme-studio .cts-windows-menu-bar [data-cts-menu-region="main"] {
  color: var(--cts-windows-main-foreground) !important;
  -webkit-text-fill-color: var(--cts-windows-main-foreground) !important;
}`;
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
  // Disable the old menu rule before its variables are cleared so the first
  // pass of a hot-switched theme measures the shell's real base padding.
  document.querySelectorAll(`.${WINDOWS_MENU_CLASS}`)
    .forEach((node) => node.classList.remove(WINDOWS_MENU_CLASS));
  document.querySelectorAll(`[${WINDOWS_MENU_REGION_ATTR}]`)
    .forEach((node) => node.removeAttribute(WINDOWS_MENU_REGION_ATTR));
  if (previous?.appliedVars) {
    for (const name of previous.appliedVars) document.documentElement?.style.removeProperty(name);
  }
  // A hot theme switch reuses the live DOM. Clear semantic annotations from
  // the previous payload before the new theme decides which glyphs it can
  // actually render; otherwise partial icon sets inherit stale hidden paths.
  document.querySelectorAll("[data-cts-glyph]").forEach((node) => node.removeAttribute("data-cts-glyph"));
  document.querySelectorAll("[data-cts-icon]").forEach((node) => node.removeAttribute("data-cts-icon"));
  document.querySelectorAll("[data-cts-logo]").forEach((node) => node.removeAttribute("data-cts-logo"));

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
    { icon: "plugins", texts: ["插件", "Plugins", "技能", "Skills"] },
    { icon: "sites", texts: ["站点", "Sites"] },
    { icon: "pull-request", texts: ["拉取请求", "Pull request"] },
    { icon: "chat", texts: ["聊天", "Chat"] },
    { icon: "settings", texts: ["设置", "Settings"] },
  ];
  const CARD_ICONS = ["explore", "build", "review", "fix"];
  const EXTENDED_ICONS = new Set(["settings", "folder"]);
  const PROJECT_ROW_SELECTOR = "[data-project-row], [data-app-action-sidebar-project-row]";

  // ── Workspace wordmark → logo variant ─────────────────────────────────────
  // Structural class names (app-shell-left-panel, main-surface, …) have held
  // stable across releases, so the CSS layer is version-robust. What drifts is
  // the workspace *wordmark* — the 2026-07 ChatGPT rebrand renamed it from
  // "ChatGPT 工作" / "ChatGPT Work" (Codex ≤ 26.707) to a bare "ChatGPT"
  // (26.715+). The styled logo art is baked with the words it shows, so the
  // OLD art must keep serving old clients while regenerated art serves new
  // ones. The choice is driven by the Codex *version*, not the wordmark text:
  // the text is localized (工作/Work/…) but the version is not, so version is
  // the robust, locale-independent signal for which art matches the shell.
  // Text below only *locates* the workspace button and splits personal(Codex)
  // vs work(ChatGPT) — it never selects the old/new art. Adapting to a future
  // wordmark change = one more boundary here plus its regenerated art, never a
  // per-user migration; the CSS falls `chatgpt` back to `chatgpt-work` art so
  // a theme that hasn't shipped the new art yet degrades instead of blanking.
  const codexVersion = () => {
    try {
      const v = window.electronBridge?.getSentryInitOptions?.()?.appVersion;
      return typeof v === "string" && /^\d+\./.test(v) ? v : null;
    } catch {
      return null;
    }
  };
  const versionAtLeast = (version, floor) => {
    const a = String(version).split(".");
    const b = floor.split(".");
    for (let i = 0; i < b.length; i += 1) {
      const d = (parseInt(a[i], 10) || 0) - (parseInt(b[i], 10) || 0);
      if (d !== 0) return d > 0;
    }
    return true;
  };
  // The work-wordmark art for THIS Codex. Undetected version → current-
  // generation art (`chatgpt`), which the CSS degrades to `chatgpt-work` when
  // a theme hasn't shipped the regenerated asset.
  const WORK_LOGO = (() => {
    const version = codexVersion();
    return version && !versionAtLeast(version, "26.715") ? "chatgpt-work" : "chatgpt";
  })();
  const workspaceLogo = (text) => {
    if (/^Codex$/i.test(text)) return "codex";
    if (/^ChatGPT( ?(工作|Work))?$/i.test(text)) return WORK_LOGO;
    return null;
  };
  const isWorkspaceTitle = (text) => workspaceLogo(text) !== null;

  // settings/folder annotation was added after the original 14-glyph runtime.
  // Gate those two on an explicit theme rule so older themes that hide native
  // paths without mapping the new glyphs never render blank controls.
  const hasExplicitGlyphStyle = (icon) => {
    const escaped = icon.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    return new RegExp(`data-cts-glyph\\s*=\\s*["']${escaped}["']`).test(cssText);
  };
  const SUPPORTED_EXTENDED_ICONS = new Set(
    [...EXTENDED_ICONS].filter((icon) => hasExplicitGlyphStyle(icon))
  );

  const glyphTarget = (container, icon) => {
    if (!container) return null;
    // Current Codex project rows expose a stable icon slot. Prefer it over the
    // generic first-svg fallback so disclosure/menu glyphs are never themed.
    if (icon === "folder") {
      const projectGlyph = container.querySelector(
        '[data-sidebar-project-drop-zone="project-icon"] svg'
      );
      if (projectGlyph) return projectGlyph;
    }
    return container.querySelector("svg");
  };

  // React may replace the svg inside an otherwise-stable control (project
  // expand/collapse does exactly this). Treat the container annotation as a
  // cache, not proof: repair it whenever the current glyph lost its marker.
  const tagGlyph = (container, icon) => {
    if (!container) return;
    if (EXTENDED_ICONS.has(icon) && !SUPPORTED_EXTENDED_ICONS.has(icon)) return;
    const svg = glyphTarget(container, icon);
    if (!svg) return;
    const tagged = [...container.querySelectorAll("svg[data-cts-glyph]")];
    const healthy = container.dataset.ctsIcon === icon
      && svg.dataset.ctsGlyph === icon
      && tagged.every((candidate) => candidate === svg);
    if (healthy) return;
    for (const candidate of tagged) {
      if (candidate !== svg) candidate.removeAttribute("data-cts-glyph");
    }
    if (container.dataset.ctsIcon !== icon) container.dataset.ctsIcon = icon;
    if (svg.dataset.ctsGlyph !== icon) svg.dataset.ctsGlyph = icon;
  };

  const clearGlyph = (container) => {
    if (!container) return;
    if (container.dataset.ctsIcon) delete container.dataset.ctsIcon;
    for (const svg of container.querySelectorAll("svg[data-cts-glyph]")) {
      svg.removeAttribute("data-cts-glyph");
    }
  };

  const projectSemantic = (control) => {
    const className = typeof control?.className === "string" ? control.className : "";
    return [
      control?.getAttribute?.("aria-label") || "",
      control?.getAttribute?.("data-testid") || "",
      control?.getAttribute?.("title") || "",
      className,
    ].join(" ");
  };

  const isProjectControl = (control) => Boolean(
    control?.matches?.(PROJECT_ROW_SELECTOR) ||
    control?.closest?.(PROJECT_ROW_SELECTOR) ||
    /(?:folder|project[-_\s]?(?:row|item|link|button)|文件夹)/i.test(projectSemantic(control))
  );

  // Project expand/collapse replaces its inner SVG during React's commit.
  // MutationObserver callbacks run before Chromium paints that commit, so
  // repair just this cheap annotation synchronously and avoid one native-icon
  // frame. The full ensure() pass remains debounced below for heavier work.
  const repairProjectGlyphs = (node) => {
    const element = node?.nodeType === 1 ? node : node?.parentElement;
    if (!element) return;
    const rows = new Set();
    const closest = element.closest?.(PROJECT_ROW_SELECTOR);
    if (closest) rows.add(closest);
    if (element.matches?.(PROJECT_ROW_SELECTOR)) rows.add(element);
    for (const row of element.querySelectorAll?.(PROJECT_ROW_SELECTOR) || []) rows.add(row);
    for (const row of rows) tagGlyph(row, "folder");
  };

  const annotateIcons = () => {
    const aside = document.querySelector(".app-shell-left-panel");
    if (aside) {
      for (const button of aside.querySelectorAll("button:not([data-cts-icon])")) {
        const text = (button.textContent || "").replace(/\s+/g, " ").trim();
        if (isWorkspaceTitle(text)) {
          clearGlyph(button);
          continue;
        }
        if (isProjectControl(button)) continue;
        const rule = SIDEBAR_ICONS.find((entry) => entry.texts.some((t) =>
          text === t || text.startsWith(`${t} `) || text.startsWith(`${t}⌘`)
        ));
        if (rule) tagGlyph(button, rule.icon);
      }
      const search = [...aside.querySelectorAll(
        '[aria-label="搜索"]:not([data-cts-icon]), [aria-label="Search"]:not([data-cts-icon])'
      )].find((control) => !isProjectControl(control));
      if (search) tagGlyph(search, "search");
      const settings = [...aside.querySelectorAll(
        '[aria-label="设置"]:not([data-cts-icon]), [aria-label="Settings"]:not([data-cts-icon])'
      )].find((control) => !isProjectControl(control));
      if (settings) tagGlyph(settings, "settings");
      // Project rows have changed element type across Codex releases (button,
      // anchor and role=button have all shipped). Match their stable semantic
      // attributes instead of project names or SVG path data, then decorate
      // only the first glyph so disclosure chevrons remain native.
      for (const control of aside.querySelectorAll(
        'button, a, [role="button"], [data-project-row], [data-app-action-sidebar-project-row]'
      )) {
        const controlText = (control.textContent || "").replace(/\s+/g, " ").trim();
        if (isWorkspaceTitle(controlText)) continue;
        const row = control.closest(PROJECT_ROW_SELECTOR);
        if (row && row !== control) continue;
        if (isProjectControl(control)) tagGlyph(control, "folder");
      }
      // Workspace title → theme-specific logo. Text can be split across child
      // spans and swaps on workspace switch, so match the whole button every
      // pass. The active UI profile owns the text→variant mapping, absorbing
      // wordmark drift (e.g. the 26.715 "ChatGPT 工作"→"ChatGPT" rebrand).
      for (const button of aside.querySelectorAll("button")) {
        const text = button.textContent.replace(/\s+/g, " ").trim();
        const want = workspaceLogo(text);
        if (!want) {
          if (button.dataset.ctsLogo) delete button.dataset.ctsLogo;
          continue;
        }
        clearGlyph(button);
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

  // Codex 26.715+ renders the Windows application menu (File/Edit/View/Help)
  // as a separate 36px flex item above the sidebar/main row. Theme CSS written
  // for the older in-main toolbar cannot reach that strip, so the stock canvas
  // shows through. Move only this structurally verified menu out of flex flow,
  // then use equivalent top padding on the real sidebar/main surfaces: their
  // own theme backgrounds extend behind the menu without cloning per-theme
  // artwork or changing any content geometry.
  const integrateWindowsMenu = (shellMain) => {
    const menu = document.querySelector(
      '.app-header-tint[class~="group/application-menu-top-bar"]'
    );
    const shellRow = menu?.nextElementSibling;
    const sidebar = shellRow?.querySelector(":scope > aside.app-shell-left-panel");
    const main = shellRow?.querySelector(":scope > main.main-surface");
    const menuBox = menu?.getBoundingClientRect();
    const integrated = Boolean(menu?.classList.contains(WINDOWS_MENU_CLASS));
    const eligible = Boolean(
      menu && sidebar && main && main === shellMain &&
      menuBox && menuBox.width > 0 && menuBox.height > 0
    );

    for (const stale of document.querySelectorAll(`.${WINDOWS_MENU_CLASS}`)) {
      if (!eligible || stale !== menu) stale.classList.remove(WINDOWS_MENU_CLASS);
    }
    for (const stale of document.querySelectorAll(`[${WINDOWS_MENU_REGION_ATTR}]`)) {
      if (!eligible || !menu.contains(stale)) stale.removeAttribute(WINDOWS_MENU_REGION_ATTR);
    }
    if (!eligible) return;

    const sidebarStyle = getComputedStyle(sidebar);
    const mainStyle = getComputedStyle(main);
    const appliedOffset = integrated ? menuBox.height : 0;
    const basePadding = (style) =>
      `${Math.max(0, (Number.parseFloat(style.paddingTop) || 0) - appliedOffset)}px`;
    setVar("--cts-windows-menu-height", `${menuBox.height}px`);
    setVar("--cts-windows-sidebar-padding-top", basePadding(sidebarStyle));
    setVar("--cts-windows-main-padding-top", basePadding(mainStyle));
    setClass(menu, WINDOWS_MENU_CLASS, true);
    setVar("--cts-windows-sidebar-foreground", sidebarStyle.color);
    setVar("--cts-windows-main-foreground", mainStyle.color);

    const sidebarRight = sidebar.getBoundingClientRect().right;
    for (const control of menu.querySelectorAll("button, [role=button]")) {
      const box = control.getBoundingClientRect();
      const region = box.left + box.width / 2 <= sidebarRight ? "sidebar" : "main";
      setAttr(control, WINDOWS_MENU_REGION_ATTR, region);
    }
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
      style.textContent = `${cssText}\n\n${RUNTIME_CSS}`;
      style.dataset.ctsStamp = STAMP;
    }

    const shellMain = document.querySelector("main.main-surface") || document.querySelector("main");
    integrateWindowsMenu(shellMain);
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
    document.querySelectorAll(`.${WINDOWS_MENU_CLASS}`).forEach((node) => node.classList.remove(WINDOWS_MENU_CLASS));
    document.querySelectorAll(`[${WINDOWS_MENU_REGION_ATTR}]`).forEach((node) => node.removeAttribute(WINDOWS_MENU_REGION_ATTR));
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
      if (mutation.type === "childList") {
        repairProjectGlyphs(target);
        for (const added of mutation.addedNodes) repairProjectGlyphs(added);
      }
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
