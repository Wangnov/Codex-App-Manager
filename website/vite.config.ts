import { defineConfig, type Plugin } from "vite";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

// Each HTML shell is filled from site/render.mjs at dev and build time, so
// every language ships as complete static markup (no runtime text swapping).
const PAGES: Record<string, "zh" | "en" | "404"> = {
  "/index.html": "zh",
  "/en/index.html": "en",
  "/404.html": "404",
};

function sitePages(): Plugin {
  return {
    name: "site-pages",
    transformIndexHtml: {
      order: "pre",
      async handler(html, ctx) {
        const page = PAGES[ctx.path];
        if (!page) throw new Error(`no page registered for ${ctx.path}`);
        const mod = ctx.server
          ? await ctx.server.ssrLoadModule("/site/render.mjs")
          : await import(pathToFileURL(resolve(import.meta.dirname, "site/render.mjs")).href);
        return mod.renderPage(html, page);
      },
    },
    handleHotUpdate({ file, server }) {
      if (file.includes("/site/")) {
        server.ws.send({ type: "full-reload" });
        return [];
      }
    },
  };
}

export default defineConfig({
  base: "/",
  appType: "mpa",
  plugins: [sitePages()],
  build: {
    target: "es2020",
    assetsInlineLimit: 0,
    rollupOptions: {
      input: {
        main: resolve(import.meta.dirname, "index.html"),
        en: resolve(import.meta.dirname, "en/index.html"),
        notFound: resolve(import.meta.dirname, "404.html"),
      },
    },
  },
});
