import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

import { JSDOM } from "jsdom";
import { test } from "vitest";

import {
  clearComposerSurfaceCompat,
  reconcileComposerSurfaces,
  selectComposerSurfaces,
} from "./composer-overflow.mjs";

const ATTR = "data-cts-composer-surface-compat";
const LEGACY = "composer-surface-chrome";
const runtimeDir = path.join(process.cwd(), "crates/codex-theme-engine/src/runtime");
const editor = '<div class="ProseMirror" data-codex-composer contenteditable="true"></div>';
// Surface attribute contract first found in the real 26.730.61309 package,
// with the editor ancestry captured from the installed 26.901.31953 build.
// The CSS-module hash deliberately differs from either audited build.
const current = `
  <div data-codex-composer-root data-composer-placement="thread">
    <div class="relative">
      <div id="surface" class="_ComposerLayoutRoot_future_9"
        data-composer-layout="multiline" data-composer-surface-variant="default"
        data-composer-surface-overflow="auto">
        <div data-composer-layout="multiline"><div class="contents">
          <div data-composer-layout="multiline" class="flex-grow overflow-y-auto">
            <div class="rich-input" style="overflow-y:auto;max-height:180px">${editor}</div>
          </div>
        </div></div>
        <button class="size-token-button-composer"><svg></svg></button>
      </div>
    </div>
  </div>`;

function domFor(html = current) {
  const dom = new JSDOM(
    `<!doctype html><html><head></head><body>
      <aside class="app-shell-left-panel"></aside>
      <main data-app-shell-main-surface>${html}</main>
    </body></html>`,
    { pretendToBeVisual: true, runScripts: "outside-only" },
  );
  dom.window.matchMedia = () => ({ matches: false, addEventListener() {}, removeEventListener() {} });
  return dom;
}

function runtimeExpression(stamp = "test:one") {
  const helpers = fs.readFileSync(path.join(runtimeDir, "composer-overflow.mjs"), "utf8")
    .replaceAll("export function ", "function ");
  return fs.readFileSync(path.join(runtimeDir, "theme-runtime.js"), "utf8")
    .replace("__CTS_COMPOSER_OVERFLOW_HELPERS__", `(() => { ${helpers}; return {
      clearComposerSurfaceCompat, createComposerOverflowAnnotator,
      reconcileComposerSurfaces, selectComposerSurfaces,
    }; })()`)
    .replace("__CTS_CSS_JSON__", JSON.stringify(`
      html.codex-theme-studio .composer-surface-chrome { background: rgb(14,18,14); }
      html.codex-theme-studio div:has(> .composer-surface-chrome)::before { content: ""; }
    `))
    .replace("__CTS_THEME_JSON__", JSON.stringify({ id: "fixture", colors: {}, strings: {} }))
    .replace("__CTS_CHROME_JSON__", "null")
    .replace("__CTS_MOTION_JSON__", "{}")
    .replace("__CTS_VERSION_JSON__", '"test"')
    .replace("__CTS_STAMP_JSON__", JSON.stringify(stamp));
}

test("current Composer restores legacy skin selectors without depending on a module hash", () => {
  const dom = domFor(`<div class="${LEGACY}">static review card</div>${current}`);
  try {
    const document = dom.window.document;
    const surface = document.getElementById("surface");
    assert.deepEqual(selectComposerSurfaces(document), [surface]);
    assert.equal(document.querySelector("div:has(> #surface)").matches(`div:has(> .${LEGACY})`), false);
    assert.deepEqual(reconcileComposerSurfaces(document), [surface]);
    assert.equal(surface.getAttribute(ATTR), "true");
    assert.equal(surface.parentElement.matches(`div:has(> .${LEGACY})`), true);
    assert.equal(document.querySelector(`.${LEGACY} button[class*="size-token-button-composer"]`)?.tagName, "BUTTON");
  } finally { dom.window.close(); }
});

test("native legacy surfaces survive reconciliation and cleanup unchanged", () => {
  const dom = domFor(`<div data-codex-composer-root><div id="legacy" class="${LEGACY}">${editor}</div></div>`);
  try {
    const document = dom.window.document;
    const before = document.body.innerHTML;
    assert.deepEqual(reconcileComposerSurfaces(document), [document.getElementById("legacy")]);
    clearComposerSurfaceCompat(document);
    assert.equal(document.body.innerHTML, before);
  } finally { dom.window.close(); }
});

test("a native legacy class on a current surface is not owned or removed", () => {
  const dom = domFor(current);
  try {
    const document = dom.window.document;
    const surface = document.getElementById("surface");
    surface.classList.add(LEGACY);
    reconcileComposerSurfaces(document);
    assert.equal(surface.hasAttribute(ATTR), false);
    clearComposerSurfaceCompat(document);
    assert.equal(surface.classList.contains(LEGACY), true);
  } finally { dom.window.close(); }
});

test("static current surfaces, misleading hashes, and nested wrappers get no duplicate frame", () => {
  const dom = domFor(`
    <div class="_ComposerLayoutRoot_fake">${editor}</div>
    <div data-composer-layout="multiline" data-composer-surface-variant="default">utility bar</div>
    <div id="outer" data-composer-layout="multiline" data-composer-surface-variant="default">${current}</div>
  `);
  try {
    const document = dom.window.document;
    assert.deepEqual(reconcileComposerSurfaces(document), [document.getElementById("surface")]);
    assert.equal(document.querySelectorAll(`[${ATTR}]`).length, 1);
    assert.equal(document.getElementById("outer").classList.contains(LEGACY), false);
  } finally { dom.window.close(); }
});

test("unmarked older editors still use the capability fallback", () => {
  const dom = domFor(`<div class="${LEGACY}">card</div><div id="legacy" class="${LEGACY}"><textarea></textarea></div>`);
  try {
    assert.deepEqual(selectComposerSurfaces(dom.window.document), [dom.window.document.getElementById("legacy")]);
  } finally { dom.window.close(); }
});

test("reconciliation is write-free when unchanged and repairs React class replacement", () => {
  const dom = domFor(current);
  try {
    const document = dom.window.document;
    const surface = document.getElementById("surface");
    reconcileComposerSurfaces(document);
    const observer = new dom.window.MutationObserver(() => {});
    observer.observe(document.body, { attributes: true, childList: true, subtree: true });
    reconcileComposerSurfaces(document);
    assert.deepEqual(observer.takeRecords(), []);
    surface.className = "_ComposerLayoutRoot_replaced_6";
    reconcileComposerSurfaces(document);
    assert.equal(surface.classList.contains(LEGACY), true);
    observer.disconnect();
  } finally { dom.window.close(); }
});

test("aliases are released when React repurposes the node or removes its editor", () => {
  for (const change of [
    (surface) => surface.removeAttribute("data-composer-surface-variant"),
    (surface) => surface.querySelector("[data-codex-composer]").remove(),
  ]) {
    const dom = domFor(current);
    try {
      const document = dom.window.document;
      const surface = document.getElementById("surface");
      reconcileComposerSurfaces(document);
      change(surface);
      assert.deepEqual(reconcileComposerSurfaces(document), []);
      assert.equal(surface.classList.contains(LEGACY), false);
      assert.equal(surface.hasAttribute(ATTR), false);
    } finally { dom.window.close(); }
  }
});

test("multiple active Composers reconcile independently and cleanup restores original markup", () => {
  const dom = domFor(current + current.replace('id="surface"', 'id="second"'));
  try {
    const document = dom.window.document;
    const before = document.body.innerHTML;
    assert.equal(reconcileComposerSurfaces(document).length, 2);
    assert.equal(document.querySelectorAll(`[${ATTR}]`).length, 2);
    clearComposerSurfaceCompat(document);
    assert.equal(document.body.innerHTML, before);
  } finally { dom.window.close(); }
});

test("production runtime hot-switch and removal own and release the compatibility aliases", () => {
  const dom = domFor(current);
  try {
    const document = dom.window.document;
    const surface = document.getElementById("surface");
    dom.window.eval(runtimeExpression());
    assert.equal(surface.classList.contains(LEGACY), true);
    assert.equal(surface.getAttribute("data-cts-composer-overflow"), "shell");
    assert.equal(surface.getAttribute("data-cts-composer-mode"), "scrolling");
    assert.equal(surface.querySelectorAll('[data-cts-composer-overflow="editor"]').length, 1);
    dom.window.eval(runtimeExpression("test:two"));
    assert.equal(surface.classList.contains(LEGACY), true);
    dom.window.__CODEX_THEME_STUDIO__.cleanup();
    assert.equal(surface.className, "_ComposerLayoutRoot_future_9");
    assert.equal(document.querySelector(`[${ATTR}], [data-cts-composer-overflow]`), null);
    assert.equal(document.getElementById("cts-style"), null);
  } finally { dom.window.close(); }
});

test("fallback removal expression clears only runtime-owned surface aliases", () => {
  const dom = domFor(current + `<div class="${LEGACY}" id="native">${editor}</div>`);
  try {
    reconcileComposerSurfaces(dom.window.document);
    const source = fs.readFileSync(path.join(runtimeDir, "../payload.rs"), "utf8");
    const expression = source.match(/pub const REMOVE_EXPRESSION: &str = r#"([\s\S]*?)"#;/)?.[1];
    assert.ok(expression);
    assert.equal(dom.window.eval(expression), true);
    assert.equal(dom.window.document.getElementById("surface").classList.contains(LEGACY), false);
    assert.equal(dom.window.document.getElementById("native").classList.contains(LEGACY), true);
  } finally { dom.window.close(); }
});

test("production observer reconciles a Composer mounted after a settings route", async () => {
  const dom = domFor("");
  try {
    dom.window.eval(runtimeExpression());
    dom.window.document.querySelector("main").innerHTML = current;
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.equal(dom.window.document.getElementById("surface").classList.contains(LEGACY), true);
    dom.window.__CODEX_THEME_STUDIO__.cleanup();
  } finally { dom.window.close(); }
});
