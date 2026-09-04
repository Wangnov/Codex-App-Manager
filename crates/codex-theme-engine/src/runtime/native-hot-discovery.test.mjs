import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { compileFunction, constants } from "node:vm";

import { afterEach, describe, expect, it } from "vitest";

// Execute the production Rust-embedded expression, including real ESM imports.
// Only the renderer's resource inventory and custom-protocol fetch are mocked.
const rust = await readFile(join(process.cwd(), "crates/codex-theme-engine/src/native_hot.rs"), "utf8");
const template = rust.match(/const ENSURE_API_JS_TEMPLATE: &str = r#"([\s\S]*?)"#;/)[1];
const newest = ["26.901.20858+", "26.831.21537+", "26.715", "26.707"];
const roots = [];
// Bypass Vitest's transformed module loader: production import() and fixtures
// must share Node's real ESM cache, just as they share Chromium's in Codex.
const nativeImport = compileFunction("return import(url)", ["url"], {
  importModuleDynamically: constants.USE_MAIN_CONTEXT_DEFAULT_LOADER,
});

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

// These small wrapper excerpts preserve the shapes measured in real packages;
// the RPC/native service implementations below are isolated test doubles.
const wrappers = {
  legacy: 'async function rd(e){return(await rpc(`get-setting`,{params:{key:e.key}})).value??e.default}\nasync function wr(e,t){await rpc(`set-setting`,{params:{key:e.key,value:t}})}\nexport {rd as r,wr as w};',
  "26.831.21537": 'async function QD(e){return(await KD(`get-setting`,{params:{key:e.key}})).value??e.default}\nasync function $D(e,t){await KD(`set-setting`,{params:{key:e.key,value:t}})}\nexport {QD as JRt,$D as ezt};',
  "26.901.20858": 'async function xO(e){return bX?.settings==null?(await DD(`get-setting`,{params:{key:e.key}})).value??e.default:(await bX.settings.read(e.key)).effective}\nasync function SO(e,t){if(bX?.settings!=null){await bX.settings.write(e.key,t);return}await DD(`set-setting`,{params:{key:e.key,value:t}})}\nexport {xO as r,SO as w};',
  "26.901.22334": 'async function bO(e){return bX?.settings==null?(await ED(`get-setting`,{params:{key:e.key}})).value??e.default:(await bX.settings.read(e.key)).effective}\nasync function xO(e,t){if(bX?.settings!=null){await bX.settings.write(e.key,t);return}await ED(`set-setting`,{params:{key:e.key,value:t}})}\nexport {bO as fUt,xO as vUt};',
};

function moduleSource(shape, native = true) {
  return `
export const calls = [];
const values = new Map([["appearanceTheme", "dark"]]);
async function rpc(method, {params}) {
  calls.push([method, params.key, params.value]);
  if (method === "get-setting") return {value: values.get(params.key)};
  values.set(params.key, params.value);
}
const KD = rpc, DD = rpc, ED = rpc, $rpc = rpc;
const bX = ${native ? `{
  settings: {
    async read(key) {calls.push(["native-read", key]); return {effective: values.get(key)};},
    async write(key, value) {calls.push(["native-write", key, value]); values.set(key, value);},
  },
}` : "null"};
${shape}
`;
}

async function renderer(files, { loaded = Object.keys(files), adapters = newest, failures = [], inspection = Function } = {}) {
  const root = await mkdtemp(join(tmpdir(), "cam-native-hot-"));
  roots.push(root);
  await writeFile(join(root, "package.json"), '{"type":"module"}');
  const urls = Object.fromEntries(Object.keys(files).map((name) => [name, pathToFileURL(join(root, name)).href]));
  await Promise.all(Object.entries(files).map(([name, source]) => writeFile(join(root, name), source)));
  const fetched = [];
  const window = {};
  const fetch = async (url) => {
    fetched.push(url);
    if (failures.includes(url) || failures.some((name) => urls[name] === url)) throw new Error("unreadable");
    return { ok: false, text: () => readFile(fileURLToPath(url), "utf8") };
  };
  const entries = loaded.map((name) => ({ name: urls[name] }));
  const document = {
    querySelectorAll: (selector) => selector === "script[src]"
      ? entries.map(({ name }) => ({ src: name }))
      : entries.map(({ name }) => ({ href: name })),
  };
  const script = template.replace("__CAM_ADAPTERS__", JSON.stringify(adapters));
  const run = compileFunction(`return ${script}`, ["window", "document", "performance", "fetch", "Function"], {
    importModuleDynamically: constants.USE_MAIN_CONTEXT_DEFAULT_LOADER,
  });
  return {
    run: () => run(window, document, { getEntriesByType: () => entries }, fetch, inspection),
    window,
    fetched,
    urls,
    module: (name) => nativeImport(urls[name]),
  };
}

async function assertRoundTrip(page, adapter, route) {
  expect(await page.run()).toMatchObject({ ok: true, cached: false, adapter });
  const mod = await page.module(Object.keys(page.urls).find((name) => page.urls[name] === page.window.__camThemeSettingsV1.url));
  expect(mod.calls).toEqual([]); // Discovery must never write, or even call read.
  const api = page.window.__camThemeSettingsV1;
  const before = await api.read({ key: "appearanceTheme" });
  expect(before).toBe("dark");
  await api.write({ key: "appearanceTheme" }, "light");
  expect(await api.read({ key: "appearanceTheme" })).toBe("light");
  await api.write({ key: "appearanceTheme" }, before);
  expect(await api.read({ key: "appearanceTheme" })).toBe("dark");
  expect(mod.calls.map((call) => call[0])).toEqual([
    route === "native" ? "native-read" : "get-setting",
    route === "native" ? "native-write" : "set-setting",
    route === "native" ? "native-read" : "get-setting",
    route === "native" ? "native-write" : "set-setting",
    route === "native" ? "native-read" : "get-setting",
  ]);
  const fetchedBefore = page.fetched.length;
  expect(await page.run()).toMatchObject({ ok: true, cached: true, adapter });
  expect(page.fetched).toHaveLength(fetchedBefore);
  expect(new Set(page.fetched).size).toBe(page.fetched.length);
}

describe("native hot settings discovery executes across Codex versions", () => {
  it("keeps the 26.707 eager adapter", async () => {
    const page = await renderer({ "index-707.js": moduleSource(wrappers.legacy) }, {
      adapters: ["26.707", "26.715", "26.831.21537+", "26.901.20858+"],
    });
    await assertRoundTrip(page, "26.707", "rpc");
  });

  it("keeps the 26.715 lazy setting-storage adapter", async () => {
    const page = await renderer({
      "index-715.js": 'const deps = ["./setting-storage-abc.js"];',
      "setting-storage-abc.js": moduleSource(wrappers.legacy),
    }, { loaded: ["index-715.js"], adapters: ["26.715", "26.707", "26.831.21537+", "26.901.20858+"] });
    await assertRoundTrip(page, "26.715", "rpc");
  });

  it("keeps 26.831.21537 dollar identifiers and export aliases", async () => {
    const page = await renderer({ "app-initial-old.js": moduleSource(wrappers["26.831.21537"].replace("as ezt", "as $ezt")) }, {
      adapters: ["26.831.21537+", "26.715", "26.707", "26.901.20858+"],
    });
    await assertRoundTrip(page, "26.831.21537+", "rpc");
  });

  for (const version of ["26.901.20858", "26.901.22334"]) {
    for (const native of [true, false]) {
      it(`${version} reads, switches and restores with native bridge ${native ? "present" : "absent"}`, async () => {
        const page = await renderer({ "app-initial-new.js": moduleSource(wrappers[version], native) });
        await assertRoundTrip(page, "26.901.20858+", native ? "native" : "rpc");
        if (!native) expect(await page.window.__camThemeSettingsV1.read({ key: "unset", default: "fallback" })).toBe("fallback");
      });
    }
  }

  it("falls back to the native adapter with a stale old version hint", async () => {
    const page = await renderer({ "app-initial-new.js": moduleSource(wrappers["26.901.22334"]) }, {
      adapters: ["26.831.21537+", "26.715", "26.707", "26.901.20858+"],
    });
    await assertRoundTrip(page, "26.901.20858+", "native");
  });

  it("accepts old wrappers when the version is unknown", async () => {
    const page = await renderer({ "app-initial-old.js": moduleSource(wrappers["26.831.21537"]) });
    await assertRoundTrip(page, "26.901.20858+", "rpc");
  });

  it("also follows lazy manifests for the native bridge", async () => {
    const page = await renderer({
      "index-new.js": 'const deps = ["setting-storage-new.js"];',
      "setting-storage-new.js": moduleSource(wrappers["26.901.22334"]),
    }, { loaded: ["index-new.js"] });
    await assertRoundTrip(page, "26.901.20858+", "native");
  });

  it("tolerates renamed parameters, formatting, guards and duplicate export aliases", async () => {
    const page = await renderer({ "app-initial-formatted.js": moduleSource(`
async function $read ( $setting ) {
  if (bX?.settings) return (await bX.settings.read($setting.key)).effective;
  return ( await $rpc ( 'get-setting', { params: { key: $setting.key } } ) ).value ?? $setting.default;
}
async function $write ( $setting, $value ) {
  if (bX?.settings) { await bX.settings.write($setting.key, $value); return; }
  await $rpc ( "set-setting", { params: { key: $setting.key, value: $value } } );
}
export { $read as $r, $write as $w, $read as duplicateRead, $write as duplicateWrite };
`) });
    await assertRoundTrip(page, "26.901.20858+", "native");
  });

  for (const [name, change] of [
    ["wrong key parameter", (source) => source.replace("key:e.key,value:t", "key:other.key,value:t")],
    ["wrong value parameter", (source) => source.replace("key:e.key,value:t", "key:e.key,value:other")],
    ["different RPC dispatchers", (source) => source.replace("await ED(`set-setting`", "await DD(`set-setting`")],
    ["unexported wrappers", (source) => source.replace("export {bO as fUt,xO as vUt};", "")],
    ["ambiguous readers", (source) => source + '\nexport async function extra(e){if(bX) return null;return(await ED(`get-setting`,{params:{key:e.key}})).value??e.default}'],
    ["ambiguous writers", (source) => source + '\nexport async function extra(e,t){if(bX) return;await ED(`set-setting`,{params:{key:e.key,value:t}})}'],
  ]) {
    it(`rejects ${name} without calling candidates`, async () => {
      const page = await renderer({ "app-initial-invalid.js": moduleSource(change(wrappers["26.901.22334"])) });
      expect(await page.run()).toMatchObject({ ok: false, error: expect.stringContaining("settings module not found") });
      expect(page.window.__camThemeSettingsV1).toBeUndefined();
      expect((await page.module("app-initial-invalid.js")).calls).toEqual([]);
    });
  }

  it("continues after failed fetches and imports without caching failures", async () => {
    const page = await renderer({
      "unreadable.js": "export {};",
      "bad-import.js": moduleSource(wrappers["26.901.22334"]) + '\nthrow new Error("failed import");',
      "app-initial-good.js": moduleSource(wrappers["26.901.22334"]),
    }, { failures: ["unreadable.js"] });
    await assertRoundTrip(page, "26.901.20858+", "native");
  });

  it("skips unrelated exports whose instrumented Function#toString throws", async () => {
    const page = await renderer({
      "app-initial-instrumented.js": moduleSource(wrappers["26.901.22334"]) + '\nexport function opaque() {}',
    }, {
      inspection: { prototype: { toString() {
        if (this.name === "opaque") throw new TypeError("Function.prototype.toString requires that 'this' be a Function");
        return Function.prototype.toString.call(this);
      } } },
    });
    await assertRoundTrip(page, "26.901.20858+", "native");
  });
});
