import { defineConfig } from "vite";

export default defineConfig({
  // Relative base so the bundle works on Cloudflare Pages, GitHub Pages
  // (project subpath) or any static file host without configuration.
  base: "./",
  build: {
    target: "es2020",
    assetsInlineLimit: 2048,
  },
});
