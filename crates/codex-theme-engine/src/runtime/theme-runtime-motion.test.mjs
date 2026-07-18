import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";

import { JSDOM } from "jsdom";
import { test } from "vitest";

const TEMPLATE_PATH = path.join(
  process.cwd(),
  "crates/codex-theme-engine/src/runtime/theme-runtime.js",
);
const HELPERS = `({
  createComposerOverflowAnnotator: () => {
    const annotate = () => {};
    annotate.invalidate = () => {};
    return annotate;
  },
  selectComposerSurfaces: () => [],
})`;
const CSS = `
html.codex-theme-studio {
  --cts-asset-intro: url("data:image/webp;base64,UklGRg==");
  --cts-intro-duration: 10s;
}`;

async function runtimeExpression({ stamp, themeId, videoSrc }) {
  const template = await fs.readFile(TEMPLATE_PATH, "utf8");
  return template
    .replace("__CTS_COMPOSER_OVERFLOW_HELPERS__", HELPERS)
    .replace("__CTS_CSS_JSON__", JSON.stringify(CSS))
    .replace("__CTS_THEME_JSON__", JSON.stringify({ id: themeId, colors: {}, strings: {} }))
    .replace("__CTS_CHROME_JSON__", "null")
    .replace("__CTS_MOTION_JSON__", JSON.stringify({ "intro-video": videoSrc }))
    .replace("__CTS_VERSION_JSON__", JSON.stringify("test"))
    .replace("__CTS_STAMP_JSON__", JSON.stringify(stamp));
}

function createDom() {
  const dom = new JSDOM(
    "<!doctype html><html><head></head><body><main data-app-shell-main-surface></main></body></html>",
    { pretendToBeVisual: true, runScripts: "outside-only" },
  );
  dom.window.matchMedia = () => ({
    matches: false,
    addEventListener() {},
    removeEventListener() {},
  });
  Object.defineProperty(dom.window.HTMLMediaElement.prototype, "play", {
    configurable: true,
    value: () => Promise.resolve(),
  });
  return dom;
}

test("hot switching replaces the previous theme's video and ignores its stale callback", async () => {
  const dom = createDom();
  try {
    dom.window.eval(await runtimeExpression({
      stamp: "theme-a:1",
      themeId: "theme-a",
      videoSrc: "data:video/mp4;base64,QQ==",
    }));
    const oldIntro = dom.window.document.getElementById("cts-intro");
    const oldVideo = oldIntro?.querySelector("video");
    assert.ok(oldIntro && oldVideo);

    dom.window.eval(await runtimeExpression({
      stamp: "theme-b:1",
      themeId: "theme-b",
      videoSrc: "data:video/mp4;base64,Qg==",
    }));
    const currentIntro = dom.window.document.getElementById("cts-intro");
    const currentVideo = currentIntro?.querySelector("video");
    assert.ok(currentIntro && currentVideo);
    assert.notEqual(currentIntro, oldIntro);
    assert.equal(currentVideo.getAttribute("src"), "data:video/mp4;base64,Qg==");

    oldVideo.dispatchEvent(new dom.window.Event("error"));
    assert.equal(dom.window.document.getElementById("cts-intro"), currentIntro);
    assert.equal(currentIntro.querySelector("video"), currentVideo);
  } finally {
    dom.window.close();
  }
});

test("a failed video remounts a fresh static intro", async () => {
  const dom = createDom();
  try {
    dom.window.eval(await runtimeExpression({
      stamp: "theme-a:1",
      themeId: "theme-a",
      videoSrc: "data:video/mp4;base64,QQ==",
    }));
    const videoIntro = dom.window.document.getElementById("cts-intro");
    const video = videoIntro?.querySelector("video");
    assert.ok(videoIntro && video);

    video.dispatchEvent(new dom.window.Event("error"));
    const fallback = dom.window.document.getElementById("cts-intro");
    assert.ok(fallback);
    assert.notEqual(fallback, videoIntro);
    assert.equal(fallback.querySelector("video"), null);
    assert.equal(fallback.dataset.ctsVideoError, "media error");
  } finally {
    dom.window.close();
  }
});
