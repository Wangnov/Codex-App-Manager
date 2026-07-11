import react from "@vitejs/plugin-react";
import { configDefaults, defineConfig } from "vitest/config";

import pkg from "./package.json";

export default defineConfig({
  plugins: [react()],
  define: {
    "import.meta.env.VITE_APP_VERSION": JSON.stringify(pkg.version),
  },
  clearScreen: false,
  test: {
    exclude: [...configDefaults.exclude, ".claude/**"],
    environment: "jsdom",
    // The suite mixes jsdom-heavy UI tests with release tests that create real
    // Git repositories. Bounding file workers avoids CPU starvation turning
    // Testing Library's behavioral waits into false timeout failures.
    maxWorkers: 4,
    environmentOptions: {
      jsdom: {
        url: "http://localhost/",
      },
    },
    setupFiles: ["./vitest.setup.ts"],
    globals: false,
  },
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
  },
});
