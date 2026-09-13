#!/usr/bin/env node
// PostToolUse/Edit|Write: fast, single-file feedback instead of waiting for
// CI to notice. Never runs the full-repo conventions script here - that
// stays a CI-only check; this only lints the one file that just changed.
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const input = JSON.parse(readFileSync(0, "utf8"));
const filePath = input.tool_input?.file_path;
if (!filePath) process.exit(0);

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
);
const relative = path.relative(repoRoot, filePath);
const inScope = /^(src|electron)\//.test(relative);
const lintable = /\.(ts|tsx|cjs|mjs|js)$/.test(relative);
if (!inScope || !lintable) process.exit(0);

const eslintBin = path.join(repoRoot, "node_modules", ".bin", "eslint");
const result = spawnSync(eslintBin, [filePath], {
  cwd: repoRoot,
  encoding: "utf8",
});

// ESLint exits 1 only when it found an error (not merely a warning).
if (result.status === 1) {
  console.log(
    JSON.stringify({
      decision: "block",
      reason: `eslint found an error in the file you just touched:\n\n${result.stdout}`,
    }),
  );
}
process.exit(0);
