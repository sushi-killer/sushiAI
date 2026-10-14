// `terminal-perf.mjs --compare-base`: the same benchmark on the base commit and
// on this tree, so before and after come from one machine and one config. The
// base runs in a temporary git worktree at $ORCHD_BASE_SHA that borrows this
// tree's node_modules and target (the daemon binary) and is removed after.
// The base may predate the benchmark, so this tree's harness is copied over
// it and measures the base's product code.
// Prints the before/after table and the gate; resolves false when a gate fails.
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  unlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { execFileSync, spawn } from "node:child_process";

const log = (...args) => console.error("[terminal-perf]", ...args);
const git = (cwd, args) =>
  execFileSync("git", args, { cwd, encoding: "utf8", stdio: "pipe" }).trim();

// xterm's scrollback (10000) plus a screen of rows.
const BUFFER_CAP_LINES = 10200;
// The benchmark itself; everything it imports besides these is product code.
const HARNESS = [
  "scripts/terminal-perf.mjs",
  "scripts/lib/perf-compare-base.mjs",
  "tests/terminal-perf-harness.tsx",
];

function runBenchmark(dir, args, output) {
  return new Promise((resolve, reject) => {
    const child = spawn(
      process.execPath,
      [path.join(dir, "scripts", "terminal-perf.mjs"), ...args],
      {
        cwd: dir,
        env: { ...process.env, PERF_OUTPUT: output },
        stdio: ["ignore", "ignore", "inherit"],
      },
    );
    child.once("error", reject);
    child.once("exit", (code, signal) =>
      code === 0
        ? resolve(JSON.parse(readFileSync(output, "utf8")))
        : reject(
            new Error(`the benchmark in ${dir} failed (${signal ?? code})`),
          ),
    );
  });
}

function gate(before, after) {
  const b = { renderer: {}, input: {}, ...before };
  const a = { renderer: {}, input: {}, ...after };
  const rtt = a.config?.rttMs;
  return [
    [
      "same config on both sides",
      JSON.stringify(a.config) === JSON.stringify(b.config),
    ],
    [
      "renderer blocked time or frame p95 at most 0.85x before",
      (b.renderer.blockedMs > 0 &&
        a.renderer.blockedMs <= 0.85 * b.renderer.blockedMs) ||
        a.renderer.frameP95Ms <= 0.85 * b.renderer.frameP95Ms,
    ],
    [
      "remote keystroke p95 at most 0.5x before",
      a.input.remote?.p95Ms <= 0.5 * b.input.remote?.p95Ms,
    ],
    [
      "remote keystroke p95 within local p95 + 1.5 RTT",
      a.input.remote?.p95Ms <= a.input.local?.p95Ms + 1.5 * rtt,
    ],
    [
      `buffer stays capped (at most ${BUFFER_CAP_LINES} lines)`,
      a.renderer.bufferLines <= BUFFER_CAP_LINES,
    ],
  ];
}

/** `args` go to both runs (for example --quick or --only); `table` prints the
 * markdown table of two result files. */
export async function compareBase({ root, args, table }) {
  const sha = process.env.ORCHD_BASE_SHA;
  if (!sha) throw new Error("--compare-base needs ORCHD_BASE_SHA");
  const temp = mkdtempSync(path.join(tmpdir(), "terminal-perf-base-"));
  const tree = path.join(temp, "base");
  const links = ["node_modules", "target"].map((name) => [
    path.join(root, name),
    path.join(tree, name),
  ]);
  let added = false;
  try {
    git(root, ["worktree", "add", "--detach", tree, sha]);
    added = true;
    mkdirSync(path.join(root, "target"), { recursive: true });
    for (const [target, link] of links) symlinkSync(target, link);
    for (const file of HARNESS) {
      mkdirSync(path.dirname(path.join(tree, file)), { recursive: true });
      copyFileSync(path.join(root, file), path.join(tree, file));
    }
    log(`base ${sha.slice(0, 12)} in ${tree}`);
    const beforeFile = path.join(temp, "before.json");
    const afterFile = path.join(temp, "after.json");
    const before = await runBenchmark(tree, args, beforeFile);
    log("this tree");
    const after = await runBenchmark(root, args, afterFile);
    table(beforeFile, afterFile);
    const checks = gate(before, after);
    console.log("");
    for (const [label, ok] of checks)
      console.log(`- [${ok ? "x" : " "}] ${label}`);
    return checks.every(([, ok]) => ok);
  } finally {
    removeBase({ root, temp, tree, links, added });
  }
}

function removeBase({ root, temp, tree, links, added }) {
  // Drop the links first so removing the worktree cannot reach their targets.
  for (const [, link] of links)
    try {
      unlinkSync(link);
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  let removed = false;
  if (added)
    try {
      git(root, ["worktree", "remove", "--force", tree]);
      removed = true;
    } catch (error) {
      log("git worktree remove failed:", error.message);
    }
  rmSync(temp, { recursive: true, force: true });
  if (added && !removed) git(root, ["worktree", "prune"]);
}
