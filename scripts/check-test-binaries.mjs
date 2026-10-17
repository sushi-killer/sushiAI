// Checks the selector against what cargo actually built: every test binary in
// `cargo nextest list --message-format json` must be one scripts/lib/rust-graph.mjs
// knows (else a change could skip a binary it cannot place), and a non-empty
// filterset must select at least one test (else `--no-tests=pass` would hide a
// filter that matches nothing). The exit code is the verdict.
// Usage: node scripts/check-test-binaries.mjs <nextest-list.json> [filterset]
import fs from "node:fs";
import { fileURLToPath } from "node:url";
import { testBinaries } from "./lib/rust-graph.mjs";

/** Problems found in a nextest list for a filterset ("" = everything). */
export function problems(
  list,
  filter,
  known = testBinaries().map((b) => b.id),
) {
  const suites = list["rust-suites"] ?? {};
  const out = Object.keys(suites)
    .filter((id) => !known.includes(id))
    .map((id) => `test binary ${id} is unknown to scripts/lib/rust-graph.mjs`);
  const selected = Object.values(suites).flatMap((s) =>
    Object.values(s.testcases ?? {}).filter(
      (t) => t["filter-match"]?.status === "matches",
    ),
  ).length;
  if (filter && selected === 0)
    out.push(`the filterset selects no test: ${filter}`);
  return out;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [file, filter = ""] = process.argv.slice(2);
  const found = problems(JSON.parse(fs.readFileSync(file, "utf8")), filter);
  for (const p of found) console.error(`::error::${p}`);
  process.exit(found.length ? 1 : 0);
}
