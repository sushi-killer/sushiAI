import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactPlugin from "eslint-plugin-react";
import reactHooks from "eslint-plugin-react-hooks";
import eslintConfigPrettier from "eslint-config-prettier";
import globals from "globals";

// Shared across the plain-JS buckets (electron/scripts/tests): `catch {}` is
// this codebase's established best-effort-cleanup idiom, and destructuring a
// key only to exclude it from a `...rest` spread is a legitimate pattern, not
// a dead binding.
const PLAIN_JS_RULES = {
  "no-empty": ["error", { allowEmptyCatch: true }],
  "no-unused-vars": ["error", { ignoreRestSiblings: true }],
};
// Control characters are the point in code that frames or strips terminal
// escape sequences - not a smuggled payload to flag.
const TERMINAL_CONTROL_CHAR_FILES = [
  "electron/agents/hermes-media.cjs",
  "electron/agents/hermes-transport.cjs",
  "electron/ipc/terminals.cjs",
  "electron/terminal-stream.cjs",
  "scripts/stream-integration.cjs",
  "scripts/terminal-rendering.mjs",
  "tests/terminal-scroll.test.cjs",
];

export default tseslint.config(
  {
    ignores: [
      "dist/**",
      "release/**",
      "node_modules/**",
      "artifacts/**",
      "concepts/**",
      ".runtime/**",
      "public/**",
      ".claude/worktrees/**",
    ],
  },
  // React/TypeScript sources: the renderer, where hook misuse actually bites.
  {
    files: ["src/**/*.{ts,tsx}"],
    extends: [...tseslint.configs.recommended],
    plugins: { react: reactPlugin, "react-hooks": reactHooks },
    languageOptions: {
      globals: { ...globals.browser },
    },
    settings: { react: { version: "detect" } },
    rules: {
      ...reactPlugin.configs.flat.recommended.rules,
      ...reactPlugin.configs.flat["jsx-runtime"].rules,
      // Only the two classic hook-correctness rules: this plugin's v7
      // "recommended" preset also pulls in the React Compiler readiness
      // rules (refs, set-state-in-effect, purity, ...), which would demand
      // refactoring idiomatic code well beyond what a lint rollout should
      // decide on its own.
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      // A condition used as a statement (`cond ? a() : b()`, `cond && a()`)
      // is this codebase's established style for a two-way or guarded call;
      // it isn't a forgotten assignment.
      "no-unused-expressions": "off",
      "@typescript-eslint/no-unused-expressions": [
        "error",
        { allowShortCircuit: true, allowTernary: true },
      ],
      // Cosmetic-only: JSX text renders a raw apostrophe correctly, and
      // rewriting UI copy to satisfy an HTML-entity preference risks
      // introducing the very typo this rule can't otherwise catch.
      "react/no-unescaped-entities": "off",
    },
  },
  // Electron main/preload process: plain Node CommonJS, no React, no
  // type-aware TS project.
  {
    files: ["electron/**/*.cjs"],
    extends: [js.configs.recommended],
    languageOptions: {
      globals: { ...globals.node },
    },
    rules: PLAIN_JS_RULES,
  },
  // Build/dev/smoke scripts: Node, but several drive Playwright's
  // page.evaluate() with a callback that runs in the browser, not Node - so
  // both global sets apply here.
  {
    files: ["scripts/**/*.{mjs,cjs}"],
    extends: [js.configs.recommended],
    languageOptions: {
      globals: { ...globals.node, ...globals.browser },
    },
    rules: PLAIN_JS_RULES,
  },
  // Tests: node:test files, .cjs importing .ts sources via type-stripping.
  {
    files: ["tests/**/*.cjs"],
    extends: [js.configs.recommended],
    languageOptions: {
      globals: { ...globals.node },
    },
    rules: PLAIN_JS_RULES,
  },
  {
    files: TERMINAL_CONTROL_CHAR_FILES,
    rules: { "no-control-regex": "off" },
  },
  // Must be last: turns off every stylistic rule Prettier already owns.
  eslintConfigPrettier,
);
