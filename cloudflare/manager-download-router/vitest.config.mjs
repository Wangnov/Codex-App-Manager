import { defineConfig } from "vitest/config";

// Local config so vitest resolves to THIS package and does not walk up to the
// repo-root vite.config.ts (which imports @vitejs/plugin-react — a main-app
// devDependency absent from this worker package's own node_modules, which made
// the CI worker-test job fail with ERR_MODULE_NOT_FOUND).
export default defineConfig({
  test: {
    environment: "node",
    include: ["test/**/*.test.js"],
  },
});
