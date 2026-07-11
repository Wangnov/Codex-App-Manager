import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import jsxA11y from "eslint-plugin-jsx-a11y";
import tseslint from "typescript-eslint";

// Lint gate for the frontend: catch hook-dependency mistakes and accessibility
// regressions the way the design review surfaced them by hand. Type-aware rules
// are intentionally NOT enabled — `tsc --noEmit` already owns type correctness
// in the same CI job, and a second type-checking pass would just double the
// wall-clock for no new signal.
export default tseslint.config(
  {
    // Frontend app only. The Node-side workspaces (build scripts, the
    // Cloudflare worker, the website subproject) have their own tooling and
    // Node globals — linting them here would just spew no-undef.
    ignores: [
      "dist/",
      "node_modules/",
      "src-tauri/",
      "website/",
      "cloudflare/",
      "vendor/",
      "scripts/",
      "docs/",
      "*.config.*",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["src/**/*.{ts,tsx}"],
    plugins: {
      "react-hooks": reactHooks,
      "jsx-a11y": jsxA11y,
    },
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: { ...globals.browser },
      parserOptions: { ecmaFeatures: { jsx: true } },
    },
    rules: {
      ...jsxA11y.flatConfigs.recommended.rules,
      // Classic hook rules only — NOT the React Compiler suite that
      // eslint-plugin-react-hooks@7 bundles into `recommended`, which flags a
      // lot of deliberate patterns here (setState-in-effect for the count-up
      // easing, the manually-memoized settings saver). Type correctness is
      // owned by `tsc` in the same job.
      // exhaustive-deps stays at its conventional `warn` severity, but the lint
      // script runs with --max-warnings=0, so in CI it (and any other warning)
      // blocks. Intentional dep omissions must be an explicit
      // `// eslint-disable-next-line react-hooks/exhaustive-deps` with a reason.
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      // The codebase leans on the leading-underscore convention for deliberate
      // discards (e.g. `_resolve` in a Promise executor).
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_", caughtErrors: "none" },
      ],
    },
  },
  {
    // The setup file's triple-slash type reference is intentional (it pulls in
    // the jest-dom matcher types globally); an import wouldn't register them.
    files: ["vitest.setup.ts"],
    rules: { "@typescript-eslint/triple-slash-reference": "off" },
  },
  {
    // Test files run under Vitest globals-free (imports describe/it/expect), but
    // use jsdom + node timers; allow both realms.
    files: ["src/**/*.test.{ts,tsx}", "vitest.setup.ts"],
    languageOptions: {
      globals: { ...globals.browser, ...globals.node },
    },
  },
);
