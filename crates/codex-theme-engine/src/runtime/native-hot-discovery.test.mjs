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
const newest = ["26.1111.21308+", "26.901.20858+", "26.831.21537+", "26.715", "26.707"];
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
  "26.1021.31345": 'async function rN(e){return(await bX.settings.read(e.key)).effective}\nasync function wN(e,t){await bX.settings.write(e.key,t)}\nexport {rN as readNative,wN as writeNative};',
  nativeHelper: 'const w$=p=>p,l$=bX;\nasync function iDt(e){if(l$?.settings!=null)return(await w$(l$.settings.read(e.key))).effective;throw Error(`Settings are unavailable on this host`)}\nasync function aDt(e,t){if(l$?.settings!=null){await w$(l$.settings.write(e.key,t));return}throw Error(`Settings are unavailable on this host`)}\nexport {iDt as readNative,aDt as writeNative};',
};

function moduleSource(shape, native = true, legacyRpc = true) {
  return `
export const calls = [];
const values = new Map([["appearanceTheme", "dark"]]);
${legacyRpc ? `async function rpc(method, {params}) {
  calls.push([method, params.key, params.value]);
  if (method === "get-setting") return {value: values.get(params.key)};
  values.set(params.key, params.value);
}
const KD = rpc, DD = rpc, ED = rpc, $rpc = rpc;` : ""}
const bX = ${native ? `{
  settings: {
    async read(key) {calls.push(["native-read", key]); return {effective: values.get(key)};},
    async write(key, value) {calls.push(["native-write", key, value]); values.set(key, value);},
  },
}` : "null"};
${shape}
`;
}

async function renderer(files, {
  loaded = Object.keys(files),
  scripts = loaded,
  adapters = newest,
  failures = [],
  inspection = Function,
  delayMs = 0,
} = {}) {
  const root = await mkdtemp(join(tmpdir(), "cam-native-hot-"));
  roots.push(root);
  await writeFile(join(root, "package.json"), '{"type":"module"}');
  const urls = Object.fromEntries(Object.keys(files).map((name) => [name, pathToFileURL(join(root, name)).href]));
  await Promise.all(Object.entries(files).map(([name, source]) => writeFile(join(root, name), source)));
  const fetched = [];
  let inFlight = 0;
  let maxConcurrentFetches = 0;
  const window = {};
  const fetch = async (url) => {
    fetched.push(url);
    inFlight += 1;
    if (inFlight > maxConcurrentFetches) maxConcurrentFetches = inFlight;
    try {
      if (delayMs > 0) await new Promise((resolve) => setTimeout(resolve, delayMs));
      if (failures.includes(url) || failures.some((name) => urls[name] === url)) throw new Error("unreadable");
      return { ok: false, text: () => readFile(fileURLToPath(url), "utf8") };
    } finally {
      inFlight -= 1;
    }
  };
  const entries = loaded.map((name) => ({ name: urls[name] }));
  const scriptEntries = scripts.map((name) => ({ name: urls[name] }));
  const document = {
    querySelectorAll: (selector) => selector === "script[src]"
      ? scriptEntries.map(({ name }) => ({ src: name }))
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
    maxConcurrentFetches: () => maxConcurrentFetches,
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

  for (const shape of ["26.1021.31345", "nativeHelper"]) {
    it(`discovers ${shape} wrappers without any legacy RPC markers`, async () => {
      const source = moduleSource(wrappers[shape], true, false);
      expect(source).not.toContain("get-setting");
      expect(source).not.toContain("set-setting");
      const page = await renderer({ "app-shared-native.js": source });
      await assertRoundTrip(page, "26.901.20858+", "native");
    });
  }

  it("finds native-only wrappers with an old version hint and a lazy manifest", async () => {
    const page = await renderer({
      "index-native.js": 'const deps = ["setting-storage-native.js"];',
      "setting-storage-native.js": moduleSource(wrappers["26.1021.31345"], true, false),
    }, { loaded: ["index-native.js"], adapters: ["26.831.21537+", "26.715", "26.707", "26.901.20858+"] });
    await assertRoundTrip(page, "26.901.20858+", "native");
  });

  it("accepts formatting and renamed parameters in native-only wrappers", async () => {
    const page = await renderer({ "app-shared-formatted.js": moduleSource(`
async function $read ( $setting ) { return ( await bX . settings . read ( $setting . key ) ) . effective; }
async function $write ( $setting, $value ) { await bX . settings . write ( $setting . key, $value ); }
export { $read as $r, $write as $w };
`, true, false) });
    await assertRoundTrip(page, "26.901.20858+", "native");
  });

  for (const [name, change] of [
    ["wrong key parameter", (source) => source.replace("write(e.key,t)", "write(other.key,t)")],
    ["wrong value parameter", (source) => source.replace("write(e.key,t)", "write(e.key,other)")],
    ["different native bridges", (source) => source.replace("bX.settings.write", "other.settings.write")],
    ["raw read response", (source) => source.replace(".effective", ".value")],
    ["discarded read response", (source) => source.replace("return(await", "(await")],
    ["unexported wrappers", (source) => source.replace("export {rN as readNative,wN as writeNative};", "")],
    ["ambiguous readers", (source) => source + '\nexport async function extra(e){return(await bX.settings.read(e.key)).effective}'],
    ["ambiguous writers", (source) => source + '\nexport async function extra(e,t){await bX.settings.write(e.key,t)}'],
  ]) {
    it(`rejects native-only ${name} without calling candidates`, async () => {
      const page = await renderer({ "app-shared-invalid.js": moduleSource(change(wrappers["26.1021.31345"]), true, false) });
      expect(await page.run()).toMatchObject({ ok: false, error: expect.stringContaining("settings module not found") });
      expect(page.window.__camThemeSettingsV1).toBeUndefined();
      expect((await page.module("app-shared-invalid.js")).calls).toEqual([]);
    });
  }

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

  const rspackRxzzShape = `
async function b(e){if(a.appServices?.settings!=null)return(await (0,s.P)(a.appServices.settings.read(e.key))).effective;throw Error("Settings are unavailable on this host")}
async function L(e,t){if(a.appServices?.settings!=null)return void await (0,s.P)(a.appServices.settings.write(e.key,t));throw Error("Settings are unavailable on this host")}`;

  function rspackEntrySource({ preloaded = "cached", includeDecoyTrap = true } = {}) {
    const registerChunk = preloaded === "cached" || preloaded === "registered"
      ? `Object.assign(p.m, {
  q77y(e,t,n){n.d(t,{},{P:async function(p){return await p;}});},
  RxZZ(e,t,n){
    n.d(t,{LA:()=>b,cz:()=>L});
    var a=n("GaA7"),s=n("q77y");
    ${rspackRxzzShape}
  }
});`
      : "";
    const bootRxzz = preloaded === "cached" ? `p("RxZZ");` : "";
    const trap = includeDecoyTrap
      ? `p.m.TRAP = function trap() { throw new Error("unrelated Rspack factory must not run"); };
Object.defineProperty(l, "BROKEN", {
  enumerable: true,
  value: { exports: Object.defineProperty({}, "bad", { enumerable: true, get() { throw new Error("broken getter"); } }) }
});`
      : "";
    return `
export const calls = [];
const values = new Map([["appearanceTheme", "dark"]]);
const appServices = {
  settings: {
    async read(key) { calls.push(["native-read", key]); return { effective: values.get(key) }; },
    async write(key, value) { calls.push(["native-write", key, value]); values.set(key, value); },
  },
};
const l = {};
const u = {
  GaA7(e,t,n){ n.d(t,{appServices:()=>appServices}); }
};
function p(e){
  var a = l[e];
  if (void 0 !== a) return a.exports;
  var c = l[e] = { id: e, exports: {} };
  u[e].call(c.exports, c, c.exports, p);
  return c.exports;
}
p.m = u;
p.c = l;
p.o = (e,a) => Object.prototype.hasOwnProperty.call(e, a);
p.d = (e,a,c) => {
  for (var f in a) p.o(a, f) && !p.o(e, f) && Object.defineProperty(e, f, { enumerable: true, get: a[f] });
  for (var f in c) p.o(c, f) && !p.o(e, f) && Object.defineProperty(e, f, { enumerable: true, value: c[f] });
};
p.C = (chunk) => {
  var c = chunk.__webpack_modules__;
  for (var a in c) p.o(c, a) && (p.m[a] = c[a]);
};
${registerChunk}
${trap}
${bootRxzz}
var h = p;
export { h as __webpack_require__ };
`;
  }

  function rspackSettingsChunkSource(body = rspackRxzzShape) {
    return `
export const __rspack_esm_ids = [956370];
export const __webpack_modules__ = {
  q77y(e,t,n){n.d(t,{},{P:async function(p){return await p;}});},
  RxZZ(e,t,n){
    n.d(t,{LA:()=>b,cz:()=>L});
    var a=n("GaA7"),s=n("q77y");
    ${body}
  }
};`;
  }

  it("discovers cached 26.1111.21308 Rspack RxZZ exports without fetching all async chunks", async () => {
    const files = {
      "348410.374a9b8f98.js": rspackEntrySource({ preloaded: "cached" }),
    };
    for (let i = 0; i < 270; i += 1) {
      files[`async-${i}.js`] = `export const __rspack_esm_ids=[${i}];export const __webpack_modules__={};`;
    }
    const page = await renderer(files, {
      scripts: ["348410.374a9b8f98.js"],
    });
    await assertRoundTrip(page, "26.1111.21308+", "native");
    expect(page.fetched).toHaveLength(1);
  });

  it("instantiates registered but uncached 26.1111.21308 RxZZ modules only after static match", async () => {
    const page = await renderer({
      "357055.d960503c8e.js": rspackEntrySource({ preloaded: "registered" }),
    }, {
      scripts: ["357055.d960503c8e.js"],
    });
    await assertRoundTrip(page, "26.1111.21308+", "native");
  });

  it("discovers lazy 26.1111.21308 Rspack settings chunks and fetches 270+ chunks in bounded parallel batches", async () => {
    const files = {
      "348410.374a9b8f98.js": rspackEntrySource({ preloaded: "none" }),
    };
    for (let i = 0; i < 270; i += 1) {
      files[`async-${i}.js`] = `export const __rspack_esm_ids=[${i}];export const __webpack_modules__={};`;
    }
    files["956370.aff5cce818.js"] = rspackSettingsChunkSource();
    const page = await renderer(files, {
      scripts: ["348410.374a9b8f98.js"],
      delayMs: 3,
    });
    const outcome = await page.run();
    expect(outcome).toMatchObject({ ok: true, cached: false, adapter: "26.1111.21308+" });
    expect(page.window.__camThemeSettingsV1.url).toBe(page.urls["956370.aff5cce818.js"]);
    expect(page.maxConcurrentFetches()).toBeGreaterThan(1);
    expect(page.maxConcurrentFetches()).toBeLessThanOrEqual(16);
    const entryMod = await page.module("348410.374a9b8f98.js");
    expect(entryMod.calls).toEqual([]);
    const api = page.window.__camThemeSettingsV1;
    const before = await api.read({ key: "appearanceTheme" });
    await api.write({ key: "appearanceTheme" }, "light");
    expect(await api.read({ key: "appearanceTheme" })).toBe("light");
    await api.write({ key: "appearanceTheme" }, before);
    expect(entryMod.calls.map((c) => c[0])).toEqual(["native-read", "native-write", "native-read", "native-write"]);
  });

  it("falls back to Rspack adapter even when an older version hint probes legacy adapters first", async () => {
    const page = await renderer({
      "348410.374a9b8f98.js": rspackEntrySource({ preloaded: "cached" }),
    }, {
      scripts: ["348410.374a9b8f98.js"],
      adapters: ["26.831.21537+", "26.715", "26.707", "26.901.20858+", "26.1111.21308+"],
    });
    await assertRoundTrip(page, "26.1111.21308+", "native");
  });

  it("fetches 270+ Vite chunks with bounded concurrency instead of serially", async () => {
    const files = {};
    for (let i = 0; i < 270; i += 1) {
      files[`chunk-${i}.js`] = `export const chunk${i} = ${i};`;
    }
    files["app-shared-tail.js"] = moduleSource(wrappers["26.1021.31345"], true, false);
    const page = await renderer(files, {
      adapters: ["26.901.20858+", "26.831.21537+", "26.715", "26.707"],
      delayMs: 3,
    });
    await assertRoundTrip(page, "26.901.20858+", "native");
    expect(page.maxConcurrentFetches()).toBeGreaterThan(1);
    expect(page.maxConcurrentFetches()).toBeLessThanOrEqual(16);
  });

  for (const [name, mutate] of [
    ["wrong key parameter", (body) => body.replace("read(e.key)", "read(other.key)")],
    ["wrong value parameter", (body) => body.replace("write(e.key,t)", "write(e.key,other)")],
    ["mismatched bridge chain", (body) => body.replace("a.appServices.settings.write", "other.appServices.settings.write")],
    ["raw read response", (body) => body.replace(".effective", ".value")],
    ["discarded read return", (body) => body.replace("return(await", "(await")],
  ]) {
    it(`rejects invalid 26.1111.21308 ${name} wrappers`, async () => {
      const page = await renderer({
        "348410.js": rspackEntrySource({ preloaded: "none", includeDecoyTrap: false }),
        "956370.js": rspackSettingsChunkSource(mutate(rspackRxzzShape)),
      }, { scripts: ["348410.js"] });
      expect(await page.run()).toMatchObject({ ok: false, error: expect.stringContaining("settings module not found") });
      expect(page.window.__camThemeSettingsV1).toBeUndefined();
      expect((await page.module("348410.js")).calls).toEqual([]);
    });
  }
});
