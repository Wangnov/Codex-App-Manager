import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

import { JSDOM } from "jsdom";
import { test } from "vitest";

const runtimeDir = path.join(process.cwd(), "crates/codex-theme-engine/src/runtime");

function runtimeExpression({
  css = "",
  chrome = null,
  stamp = "compat:test",
} = {}) {
  const helpers = fs.readFileSync(path.join(runtimeDir, "composer-overflow.mjs"), "utf8")
    .replaceAll("export function ", "function ");
  return fs.readFileSync(path.join(runtimeDir, "theme-runtime.js"), "utf8")
    .replace("__CTS_COMPOSER_OVERFLOW_HELPERS__", `(() => { ${helpers}; return {
      clearComposerSurfaceCompat, createComposerOverflowAnnotator,
      reconcileComposerSurfaces, selectComposerSurfaces,
    }; })()`)
    .replace("__CTS_CSS_JSON__", JSON.stringify(css))
    .replace("__CTS_THEME_JSON__", JSON.stringify({ id: "compat", colors: {}, strings: {} }))
    .replace("__CTS_CHROME_JSON__", JSON.stringify(chrome))
    .replace("__CTS_MOTION_JSON__", "{}")
    .replace("__CTS_VERSION_JSON__", '"test"')
    .replace("__CTS_STAMP_JSON__", JSON.stringify(stamp));
}

function domFor(body) {
  const dom = new JSDOM(
    `<!doctype html><html><head></head><body>${body}</body></html>`,
    { pretendToBeVisual: true, runScripts: "outside-only" },
  );
  dom.window.matchMedia = () => ({ matches: false, addEventListener() {}, removeEventListener() {} });
  return dom;
}

function mutableRect(node, initial) {
  const value = { ...initial };
  node.getBoundingClientRect = () => ({
    x: value.x,
    y: value.y,
    left: value.x,
    top: value.y,
    width: value.width,
    height: value.height,
    right: value.x + value.width,
    bottom: value.y + value.height,
    toJSON() { return this; },
  });
  return value;
}

function cleanupDom(dom) {
  dom.window.__CODEX_THEME_STUDIO__?.cleanup();
  dom.window.close();
}

test("active main ignores retained hidden home trees and follows navigation back to them", () => {
  const dom = domFor(`
    <aside class="app-shell-left-panel"></aside>
    <div id="old-wrapper" style="content-visibility:hidden">
      <main id="old" data-app-shell-main-surface>
        <div id="old-home" role="main"><div data-testid="home-icon"></div></div>
      </main>
    </div>
    <div id="new-wrapper">
      <main id="new" data-app-shell-main-surface>
        <div id="new-chat" role="main">thread</div>
      </main>
    </div>
  `);
  try {
    const document = dom.window.document;
    const oldMain = document.getElementById("old");
    const newMain = document.getElementById("new");
    const oldHome = document.getElementById("old-home");
    const newChat = document.getElementById("new-chat");
    const oldRect = mutableRect(oldMain, { x: 300, y: 40, width: 900, height: 700 });
    const newRect = mutableRect(newMain, { x: 420, y: 50, width: 800, height: 680 });
    mutableRect(oldHome, { x: 300, y: 80, width: 900, height: 620 });
    mutableRect(newChat, { x: 420, y: 90, width: 800, height: 600 });

    dom.window.eval(runtimeExpression({
      chrome: `
        <div data-cts-layer="overlay"><i>overlay</i></div>
        <div data-cts-layer="stage"><b>stage</b></div>`,
    }));

    assert.equal(oldMain.classList.contains("main-surface"), false);
    assert.equal(newMain.getAttribute("data-cts-main-surface-compat"), "true");
    assert.equal(document.querySelector(".cts-home"), null, "hidden old home must not mark a chat");
    assert.equal(newMain.classList.contains("cts-home-shell"), false);
    assert.equal(document.getElementById("cts-stage").parentElement, newMain);
    assert.equal(document.getElementById("cts-chrome").style.left, "420px");

    document.getElementById("old-wrapper").style.contentVisibility = "visible";
    document.getElementById("new-wrapper").style.contentVisibility = "hidden";
    dom.window.__CODEX_THEME_STUDIO__.ensure();

    assert.equal(newMain.classList.contains("main-surface"), false);
    assert.equal(newMain.hasAttribute("data-cts-main-surface-compat"), false);
    assert.equal(oldMain.getAttribute("data-cts-main-surface-compat"), "true");
    assert.equal(oldHome.classList.contains("cts-home"), true);
    assert.equal(oldMain.classList.contains("cts-home-shell"), true);
    assert.equal(document.getElementById("cts-stage").parentElement, oldMain);
    assert.equal(document.getElementById("cts-stage").classList.contains("cts-home-shell"), true);
    assert.equal(document.getElementById("cts-chrome").style.left, "300px");

    document.getElementById("old-wrapper").style.contentVisibility = "hidden";
    document.getElementById("new-wrapper").style.contentVisibility = "visible";
    newRect.width = 0;
    newRect.height = 0;
    dom.window.__CODEX_THEME_STUDIO__.ensure();

    assert.equal(document.querySelector("[data-cts-main-surface-compat]"), null);
    assert.equal(document.querySelector(".cts-home, main.cts-home-shell"), null);
    assert.equal(document.getElementById("cts-stage"), null);
    assert.equal(document.getElementById("cts-chrome").style.display, "none");
    assert.equal(document.getElementById("cts-chrome").classList.contains("cts-home-shell"), false);
    assert.equal(dom.window.__CODEX_THEME_STUDIO__.homeSticky, null);

    // Keep these live bindings used so the test also documents that the
    // surfaces themselves remained measurable while their ancestors hid them.
    assert.equal(oldRect.width, 900);
  } finally {
    cleanupDom(dom);
  }
});

test("observer follows ancestor-only cached main switches without a composer", async () => {
  const dom = domFor(`
    <aside class="app-shell-left-panel"></aside>
    <div id="first-wrapper">
      <main id="first" data-app-shell-main-surface><div role="main">space</div></main>
    </div>
    <div id="second-wrapper" style="content-visibility:hidden">
      <main id="second" data-app-shell-main-surface><div role="main">review</div></main>
    </div>
  `);
  try {
    const document = dom.window.document;
    const first = document.getElementById("first");
    const second = document.getElementById("second");
    mutableRect(first, { x: 280, y: 40, width: 720, height: 620 });
    mutableRect(second, { x: 320, y: 50, width: 680, height: 600 });
    dom.window.eval(runtimeExpression({
      chrome: '<div data-cts-layer="stage"><b>stage</b></div>',
    }));
    assert.equal(document.getElementById("cts-stage").parentElement, first);

    document.getElementById("first-wrapper").style.contentVisibility = "hidden";
    document.getElementById("second-wrapper").style.contentVisibility = "visible";
    await new Promise((resolve) => dom.window.setTimeout(resolve, 260));

    assert.equal(document.getElementById("cts-stage").parentElement, second);
    assert.equal(first.hasAttribute("data-cts-main-surface-compat"), false);
    assert.equal(second.getAttribute("data-cts-main-surface-compat"), "true");
  } finally {
    cleanupDom(dom);
  }
});

test("a visible legacy main remains supported without becoming runtime-owned", () => {
  const dom = domFor(`
    <aside class="app-shell-left-panel"></aside>
    <main id="legacy" class="main-surface"><div role="main">legacy</div></main>
  `);
  try {
    const legacy = dom.window.document.getElementById("legacy");
    mutableRect(legacy, { x: 240, y: 30, width: 700, height: 600 });
    dom.window.eval(runtimeExpression({
      chrome: '<div data-cts-layer="stage"><b>stage</b></div>',
    }));
    assert.equal(legacy.classList.contains("main-surface"), true);
    assert.equal(legacy.hasAttribute("data-cts-main-surface-compat"), false);
    assert.equal(dom.window.document.getElementById("cts-stage").parentElement, legacy);
    dom.window.__CODEX_THEME_STUDIO__.cleanup();
    assert.equal(legacy.classList.contains("main-surface"), true);
  } finally {
    cleanupDom(dom);
  }
});

test("sidebar destinations are semantic, gated, reusable, and idempotent", () => {
  const button = (id, attributes, text) =>
    `<button id="${id}" ${attributes}><svg><path></path></svg><span>${text}</span></button>`;
  const dom = domFor(`
    <aside class="app-shell-left-panel">
      ${button("home", 'data-sidebar-destination="builtin:home"', "首页")}
      ${button("space", 'data-sidebar-destination="builtin:space"', "空间")}
      ${button("scheduled", 'data-sidebar-destination="builtin:automations"', "定时任务")}
      ${button("plugins", 'data-sidebar-destination="builtin:customize"', "插件")}
      ${button("review", 'data-sidebar-destination="builtin:pull-requests"', "代码审查")}
      ${button("new-chat", "", "新聊天")}
      ${button("explore", "", "探索")}
    </aside>
    <main id="main" data-app-shell-main-surface><div role="main">thread</div></main>
  `);
  try {
    const document = dom.window.document;
    mutableRect(document.getElementById("main"), { x: 300, y: 40, width: 700, height: 600 });
    const explicitCss = `
      svg[data-cts-glyph="home"] { background-image: url(home.png); }
      svg[data-cts-glyph="space"] { background-image: url(space.png); }
    `;
    dom.window.eval(runtimeExpression({ css: explicitCss, stamp: "icons:explicit" }));
    const glyph = (id) => document.querySelector(`#${id} svg`)?.dataset.ctsGlyph ?? null;
    assert.deepEqual(
      ["home", "space", "scheduled", "plugins", "review", "new-chat", "explore"].map(glyph),
      ["home", "space", "scheduled", "plugins", "pull-request", "new-task", "explore"],
    );

    const observer = new dom.window.MutationObserver(() => {});
    observer.observe(document.body, { attributes: true, childList: true, subtree: true });
    dom.window.__CODEX_THEME_STUDIO__.ensure();
    assert.deepEqual(observer.takeRecords(), [], "unchanged reconciliation must not write");
    observer.disconnect();

    document.getElementById("scheduled").setAttribute("data-sidebar-destination", "builtin:pull-requests");
    document.querySelector("#new-chat span").textContent = "探索";
    dom.window.__CODEX_THEME_STUDIO__.ensure();
    assert.equal(glyph("scheduled"), "pull-request");
    assert.equal(glyph("new-chat"), "explore");

    dom.window.eval(runtimeExpression({
      css: 'svg[data-cts-glyph="scheduled"] { background-image: url(clock.png); }',
      stamp: "icons:legacy-theme",
    }));
    assert.equal(glyph("home"), null, "an old theme must keep the native home glyph");
    assert.equal(glyph("space"), null, "an old theme must keep the native space glyph");
    assert.equal(glyph("plugins"), "plugins");
  } finally {
    cleanupDom(dom);
  }
});

test("composer actions distinguish send, stop, and voice without size guesses", () => {
  const dom = domFor(`
    <aside class="app-shell-left-panel"></aside>
    <main id="main" data-app-shell-main-surface>
      <div data-codex-composer-root>
        <div id="composer" data-composer-layout="multiline" data-composer-surface-variant="default">
          <div data-composer-layout="multiline"><div class="overflow-y-auto">
            <div data-codex-composer contenteditable="true"></div>
          </div></div>
          <button id="send" class="size-token-button-composer"><svg></svg></button>
          <button id="stop" class="size-token-button-composer" aria-label="停止"><svg></svg></button>
          <button id="voice" aria-label="Start voice mode"><svg></svg></button>
          <button id="dictation" aria-label="听写"><svg></svg></button>
          <button id="english-dictation" aria-label="Start dictation"><svg></svg></button>
          <button id="english-dictate" title="Dictate"><svg></svg></button>
          <button id="labeled-primary" class="size-token-button-composer" aria-label="Más acciones"><svg></svg></button>
          <button id="lookalike" class="size-token-button-composer-extra"><svg></svg></button>
          <button id="plain" data-size="composer"><svg></svg></button>
        </div>
      </div>
    </main>
  `);
  try {
    const document = dom.window.document;
    mutableRect(document.getElementById("main"), { x: 200, y: 40, width: 800, height: 640 });
    mutableRect(document.getElementById("composer"), { x: 300, y: 500, width: 600, height: 100 });
    dom.window.eval(runtimeExpression({
      css: 'button[data-cts-composer-action="send"] { background-image: url(starship.png); }',
      stamp: "actions:one",
    }));

    const action = (id) => document.getElementById(id).getAttribute("data-cts-composer-action");
    assert.equal(action("send"), "send", "legacy unlabeled primary action remains supported");
    assert.equal(action("stop"), "stop");
    assert.equal(action("voice"), "voice");
    assert.equal(action("dictation"), "voice");
    assert.equal(action("english-dictation"), "voice");
    assert.equal(action("english-dictate"), "voice");
    assert.equal(action("labeled-primary"), null, "an unknown labeled action must keep its native appearance");
    assert.equal(action("lookalike"), null);
    assert.equal(action("plain"), null);
    assert.match(dom.window.getComputedStyle(document.getElementById("send")).backgroundImage, /starship/);
    assert.doesNotMatch(dom.window.getComputedStyle(document.getElementById("stop")).backgroundImage, /starship/);
    assert.doesNotMatch(dom.window.getComputedStyle(document.getElementById("voice")).backgroundImage, /starship/);

    document.getElementById("send").setAttribute("aria-label", "Stop generating");
    document.getElementById("voice").setAttribute("aria-label", "More actions");
    dom.window.__CODEX_THEME_STUDIO__.ensure();
    assert.equal(action("send"), "stop", "a reused primary button must update its action");
    assert.equal(action("voice"), null, "a reused voice button must release its marker");

    const observer = new dom.window.MutationObserver(() => {});
    observer.observe(document.body, { attributes: true, subtree: true });
    dom.window.__CODEX_THEME_STUDIO__.ensure();
    assert.deepEqual(observer.takeRecords(), [], "action reconciliation must be idempotent");
    observer.disconnect();

    dom.window.__CODEX_THEME_STUDIO__.cleanup();
    assert.equal(document.querySelector("[data-cts-composer-action]"), null);
  } finally {
    cleanupDom(dom);
  }
});
