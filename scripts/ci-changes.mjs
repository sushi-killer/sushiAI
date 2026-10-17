// Which checks a change needs, from the files it changes: one selector for
// the CI workflow, `npm run ci` and `node scripts/test-rust.mjs --affected`.
// Docs alone skip the builds and tests. A crate change runs the Rust test
// binaries that can see it (scripts/lib/rust-graph.mjs) and the node tests
// that drive the Rust side; any other code change runs every node test. The
// macOS Rust tests run only when the platform can be affected (below).
// Without a usable base (a push, a new branch) or after a change to the build
// or test setup, everything runs. Prints the scope for $GITHUB_OUTPUT.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { readData } from "./ci-platform.mjs";
import { hostCrates, nextestFilter, rustSelection } from "./lib/rust-graph.mjs";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
// Markdown that tests or tools read is code: skills, agents, prompts.
const CODE_MARKDOWN =
  /^(\.agents|\.claude|crates|src|electron|scripts|tests)\//;
// The build and test setup: a change here can change what any check does,
// so everything runs.
export const EVERYTHING = [
  /^package(-lock)?\.json$/,
  /^Cargo\.(toml|lock)$/,
  /^rust-toolchain\.toml$/,
  /^\.nvmrc$/,
  /^\.config\//,
  /^tsconfig[^/]*\.json$/,
  /^eslint\.config\.mjs$/,
  /^\.github\/workflows\//,
  /^tests\/(helpers|fixtures)\//,
  /^scripts\/(ci|ci-changes|ci-platform|ci-test-map|test-rust|check-crate-deps|check-test-binaries|flaky-report|nextest-sushiai-bin|nextest-summary)\.(mjs|json)$/,
  /^scripts\/lib\/(rust-graph|run-lanes)\.mjs$/,
  // The Rust tests read these fixtures and spawn node fake harnesses.
  /^electron\/test-fixtures\/mcp-home\//,
];
// What `npm run build` (tsc and vite) reads.
const WEB = [
  /^src\//,
  /^public\//,
  /^(index|mascot)\.html$/,
  /^vite\.config\.ts$/,
];
// What the desktop smoke runs besides the scripts it starts.
const DESKTOP = [...WEB, /^electron\//];
const DESKTOP_SCRIPTS = [
  "scripts/test-desktop.mjs",
  "scripts/build-once.mjs",
  "scripts/smoke.mjs",
  "scripts/smoke-companion.mjs",
];
// A node test whose sources name one of these drives the Rust side: a real
// daemon, the crates' files or cargo. Only these run for a crate change.
const RUST_MARKERS =
  /real-daemon|daemonBinary|target\/debug\/sushiai|SUSHIAI_DAEMON_BIN|\bcrates\b|\bcargo\b/;
// The build itself: a dependency, the toolchain or the CI setup can change
// what is compiled or run on each OS.
const PLATFORM_PATHS = [
  /^Cargo\.(toml|lock)$/,
  /^crates\/[^/]+\/Cargo\.toml$/,
  /^rust-toolchain\.toml$/,
  /^\.config\//,
  /^\.github\/workflows\//,
  /^scripts\/(test-rust|nextest-sushiai-bin|nextest-summary|ci-changes|ci-platform)\.(mjs|json)$/,
  // Shell fixtures run under the OS's own sh, sed, date and stat.
  /^crates\/.*\.sh$/,
];
// Conditional compilation per OS: a file with it builds different code on
// macOS, so any edit of that file needs the macOS tests.
export const PLATFORM_CFG =
  /\bcfg!?\(\s*(?:(?:not|any|all)\(\s*)*(?:target_os|target_family|target_vendor|unix|windows)\b/;
// Code that behaves differently per OS: conditional compilation, the C and
// unix APIs, process groups and signals, procfs and sysctl, and shell tools
// whose BSD and GNU flags differ. Only the changed lines are matched: most
// files that call one of these APIs are edited for other reasons.
export const PLATFORM_CODE = new RegExp(
  [
    PLATFORM_CFG.source,
    String.raw`\blibc::`,
    String.raw`\bstd::os::(?:unix|macos|linux|fd)\b`,
    String.raw`\bportable_pty\b`,
    String.raw`\b(?:pre_exec|process_group|setsid|killpg|setpgid)\b`,
    String.raw`\btokio::signal\b`,
    String.raw`\bSIG(?:TERM|KILL|INT|HUP|CHLD|STOP|CONT)\b`,
    String.raw`/proc/`,
    String.raw`\b(?:sysctl|proc_pid\w*|launchctl|osascript)\b`,
    String.raw`\b(?:sed -i|date -d|stat -c|readlink -f|grep -P|xargs -r|base64 -w)\b`,
  ].join("|"),
);
// Shards per OS for the Rust tests: with the test durations of 140 CI runs,
// ubuntu's slowest of 2 shards and macOS's of 3 each finish in under 4
// minutes, and a third ubuntu shard saved less than its setup cost.
export const SHARDS = { "ubuntu-latest": 2, "macos-latest": 3 };

export function isDoc(file) {
  return (
    (file.endsWith(".md") && !CODE_MARKDOWN.test(file)) ||
    /^docs\/.*\.(md|png|svg)$/.test(file)
  );
}

/** The `+`/`-` lines of a unified diff, per file. */
export function diffLines(diff) {
  const lines = new Map();
  let file = null;
  for (const line of diff.split("\n")) {
    const header = /^diff --git a\/(\S+) b\/(\S+)/.exec(line);
    if (header) {
      file = header[2];
      lines.set(file, []);
    } else if (file && /^[+-](?![+-]{2} )/.test(line)) {
      lines.get(file).push(line.slice(1));
    }
  }
  return lines;
}

/**
 * Why a change needs the macOS Rust tests, or null when it does not.
 * `diff` is the unified diff of the change; `read(file)` returns a changed
 * file's current text (null when it was deleted).
 */
export function platformReason(files, { diff = "", read = () => null } = {}) {
  const data = readData();
  const hunks = diffLines(diff);
  for (const file of files) {
    // A release bumps Cargo.lock too, so a release pull request runs macOS.
    if (PLATFORM_PATHS.some((r) => r.test(file))) return `${file}: build setup`;
    const crate = data.crates.find((c) => file.startsWith(`${c}/`));
    if (crate) return `${file}: ${crate} has platform-only dependencies`;
    if (data.macosOnly.includes(file))
      return `${file}: a test here failed only on macOS`;
    if (!/^crates\/.*\.rs$/.test(file)) continue;
    const changed = (hunks.get(file) ?? []).find((l) => PLATFORM_CODE.test(l));
    if (changed) return `${file}: changes platform code: ${changed.trim()}`;
    const text = read(file);
    if (text && PLATFORM_CFG.test(text))
      return `${file}: compiles per OS (cfg)`;
  }
  return null;
}

const SOURCE = /\.[cm]?[jt]sx?$/;

// A file and every file it loads by a relative path or names from the root.
function closure(file, seen = new Set()) {
  const full = path.join(root, file);
  if (seen.has(file) || !fs.existsSync(full) || fs.statSync(full).isDirectory())
    return seen;
  seen.add(file);
  if (!SOURCE.test(file)) return seen;
  const text = fs.readFileSync(full, "utf8");
  const dir = path.posix.dirname(file);
  for (const [, spec] of text.matchAll(
    /(?:require\(|from\s+|import\()\s*["'](\.{1,2}\/[^"']+)["']/g,
  )) {
    const base = path.posix.join(dir, spec);
    const found = ["", ".ts", ".tsx", ".cjs", ".mjs", ".js", "/index.ts"]
      .map((ext) => base + ext)
      .find((f) => fs.existsSync(path.join(root, f)));
    if (found) closure(found, seen);
  }
  for (const [, named] of text.matchAll(
    /["'`]((?:src|electron|scripts|tests|crates)\/[\w./-]+\.\w+)["'`]/g,
  ))
    closure(named, seen);
  return seen;
}

let testsCache = null;
/** Every node test file, the files it loads, and whether it drives the Rust side. */
export function nodeTestGraph() {
  testsCache ??= fs
    .readdirSync(path.join(root, "tests"))
    .filter((f) => f.endsWith(".test.cjs"))
    .sort()
    .map((f) => {
      const file = `tests/${f}`;
      // The test's own text: the app code it loads talks to the daemon too,
      // but only through fakes unless the test itself starts a real one.
      const text = fs.readFileSync(path.join(root, file), "utf8");
      return { file, files: closure(file), rust: RUST_MARKERS.test(text) };
    });
  return testsCache;
}

const ALL = {
  code: true,
  rust: true,
  rustTests: null,
  nodeTests: null,
  web: true,
  desktop: true,
};

/**
 * The checks a change needs. `rustTests` and `nodeTests` are null for all of
 * them, otherwise the nextest binary ids and the node test files to run.
 * `all` says why everything runs; `reason` why the macOS Rust tests run.
 */
export function classify(files, change = {}) {
  if (files.length === 0)
    return { ...ALL, platform: true, reason: "no diff", all: "no diff" };
  const code = files.filter((f) => !isDoc(f));
  if (code.length === 0)
    return {
      code: false,
      rust: false,
      rustTests: [],
      nodeTests: [],
      web: false,
      desktop: false,
      platform: false,
      reason: null,
      all: null,
    };
  const crates = code.filter((f) => f.startsWith("crates/"));
  const other = code.filter((f) => !f.startsWith("crates/"));
  const setup = other.find((f) => EVERYTHING.some((r) => r.test(f)));
  const rs = crates.length
    ? rustSelection(crates)
    : { all: false, crates: [], binaries: [] };
  const rust = Boolean(setup) || crates.length > 0;
  const reason = rust ? platformReason(code, change) : null;
  const platform = Boolean(reason);
  const broad = setup
    ? `${setup}: build or test setup`
    : rs.unknown && rs.reason;
  if (broad) return { ...ALL, platform, reason, all: broad };

  const host = hostCrates();
  const smoke = new Set(DESKTOP_SCRIPTS.flatMap((f) => [...closure(f)]));
  const changed = new Set(code);
  return {
    code: true,
    rust,
    rustTests: rs.all ? null : rs.binaries,
    // Code outside the crates can reach any node test: all of them run.
    nodeTests: other.length
      ? null
      : nodeTestGraph()
          .filter((t) => t.rust || [...t.files].some((f) => changed.has(f)))
          .map((t) => t.file),
    web: other.some((f) => WEB.some((r) => r.test(f))),
    // The smoke starts the daemon: the host crates count, a module does not
    // (its own tests start the daemon with it).
    desktop:
      other.some((f) => DESKTOP.some((r) => r.test(f)) || smoke.has(f)) ||
      rs.all ||
      rs.crates.some((c) => host.has(c)),
    platform,
    reason,
    all: null,
  };
}

/** The Rust build and test matrices of the CI workflow for a scope. */
export function matrices({ platform }) {
  const os = Object.keys(SHARDS).filter(
    (o) => platform || o !== "macos-latest",
  );
  return {
    build: { os },
    test: {
      include: os.flatMap((o) =>
        Array.from({ length: SHARDS[o] }, (_, i) => ({
          os: o,
          shard: i + 1,
          shards: SHARDS[o],
        })),
      ),
    },
  };
}

// Quarantined Rust tests: each entry names its binary, test, owner, expiry
// date and issue (tests/ci-pipeline.test.cjs fails on a missing field or a
// past expiry, so a quarantine cannot be forgotten).
export const QUARANTINE = path.join(root, ".config", "flaky-quarantine.json");
export function readQuarantine() {
  return JSON.parse(fs.readFileSync(QUARANTINE, "utf8"));
}

/** The nextest filterset for a scope: its test binaries minus the quarantine; "" runs all. */
export function rustFilter(scope, quarantine = readQuarantine()) {
  const parts = [];
  if (scope.rustTests) parts.push(`(${nextestFilter(scope.rustTests)})`);
  for (const q of quarantine)
    parts.push(`not (binary_id(=${q.binary}) & test(=${q.test}))`);
  return parts.join(" & ");
}

/**
 * Builds of this checkout go to its own `target/` unless CARGO_TARGET_DIR is
 * set: a host-wide `build.target-dir` would share one build lock and one
 * `target/debug/sushiai` between checkouts, and the real-daemon node tests
 * look for the binary here.
 */
export function ownTargetDir() {
  process.env.CARGO_TARGET_DIR ||= path.join(root, "target");
}

/** The scope as `key=value` lines for $GITHUB_OUTPUT. */
export function outputs(scope) {
  const { build, test } = matrices(scope);
  const nodeTests = scope.nodeTests ?? ["tests/*.test.cjs"];
  return [
    `code=${scope.code}`,
    `rust=${scope.rust}`,
    `platform=${scope.platform}`,
    `rust_build=${JSON.stringify(build)}`,
    `rust_test=${JSON.stringify(test)}`,
    `rust_filter=${rustFilter(scope)}`,
    `node=${nodeTests.length > 0}`,
    `node_tests=${nodeTests.join(" ")}`,
    `web=${scope.web}`,
    `desktop=${scope.desktop}`,
  ].join("\n");
}

/** One line on what runs and why, for a log. */
export function describe(scope, files) {
  const count = (list, word) =>
    list === null ? `all ${word}` : `${list.length} ${word}`;
  const what = scope.all
    ? `everything runs (${scope.all})`
    : [
        count(scope.rustTests, "Rust test binaries"),
        count(scope.nodeTests, "node test files"),
        `web build ${scope.web ? "yes" : "no"}`,
        `desktop ${scope.desktop ? "yes" : "no"}`,
      ].join(", ");
  const mac = scope.platform ? `yes (${scope.reason})` : "no";
  return `${files} changed file(s): ${what}; macOS Rust ${mac}`;
}

const git = (...args) =>
  execFileSync("git", args, { encoding: "utf8", maxBuffer: 1 << 28 });

// What this checkout changes against the merge base with origin/main (or
// CI_BASE, for a branch stacked on another): commits, edits and new files.
// Empty when it cannot tell, so everything runs.
export function localChanges() {
  try {
    const against = process.env.CI_BASE || "origin/main";
    const base = git("merge-base", against, "HEAD").trim();
    const files = [
      ...new Set([
        ...git("diff", "--name-only", "--no-renames", base)
          .split("\n")
          .filter(Boolean),
        ...git("ls-files", "--others", "--exclude-standard")
          .split("\n")
          .filter(Boolean),
      ]),
    ];
    return { files, diff: git("diff", "-U0", "--no-renames", base), read };
  } catch {
    return { files: [] };
  }
}

function read(file) {
  try {
    return fs.readFileSync(file, "utf8");
  } catch {
    return null;
  }
}

function changes(base, head) {
  if (!base || /^0+$/.test(base)) return { files: [] };
  try {
    // Both sides of a rename, against the merge base: a moved code file counts.
    const range = `${base}...${head}`;
    const files = git("diff", "--name-only", "--no-renames", range)
      .split("\n")
      .filter(Boolean);
    return { files, diff: git("diff", "-U0", "--no-renames", range), read };
  } catch {
    return { files: [] };
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const change = changes(process.env.BASE, process.env.HEAD || "HEAD");
  const scope = classify(change.files, change);
  console.log(outputs(scope));
  console.error(describe(scope, change.files.length));
}
