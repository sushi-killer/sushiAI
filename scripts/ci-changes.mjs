// Which CI jobs a pull request needs, from the files it changes. Docs alone
// skip the builds and tests; the Rust jobs run only when Rust can be affected;
// the macOS Rust tests run only when the platform can be affected (below).
// Prints `code=`, `rust=`, `platform=` and the Rust job matrices for
// $GITHUB_OUTPUT. Without a usable base (a push, a schedule, a new branch)
// everything runs, macOS included.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import { fileURLToPath } from "node:url";
import { readData } from "./ci-platform.mjs";

// Markdown that tests or tools read is code: skills, agents, prompts.
const CODE_MARKDOWN =
  /^(\.agents|\.claude|crates|src|electron|scripts|tests)\//;
const RUST = [
  /^crates\//,
  /^Cargo\.(toml|lock)$/,
  /^rust-toolchain\.toml$/,
  /^\.config\//,
  /^\.github\/workflows\//,
  /^scripts\/(test-rust|check-crate-deps|nextest-sushiai-bin|nextest-summary|ci-changes|ci-platform)\.(mjs|json)$/,
  // The Rust tests read these fixtures and spawn node fake harnesses.
  /^electron\/test-fixtures\/mcp-home\//,
  /^\.nvmrc$/,
];
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
// Shards per OS for the Rust tests (artifacts/PLAN-ci-parallel.md has the
// measurements behind the numbers).
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
    const history = data.macosOnly.find((e) => e.files.includes(file));
    if (history) return `${file}: ${history.test} failed only on macOS`;
    if (!/^crates\/.*\.rs$/.test(file)) continue;
    const changed = (hunks.get(file) ?? []).find((l) => PLATFORM_CODE.test(l));
    if (changed) return `${file}: changes platform code: ${changed.trim()}`;
    const text = read(file);
    if (text && PLATFORM_CFG.test(text))
      return `${file}: compiles per OS (cfg)`;
  }
  return null;
}

export function classify(files, change = {}) {
  if (files.length === 0)
    return { code: true, rust: true, platform: true, reason: "no diff" };
  const code = !files.every(isDoc);
  const rust = code && files.some((f) => RUST.some((r) => r.test(f)));
  const reason = rust ? platformReason(files, change) : null;
  return { code, rust, platform: Boolean(reason), reason };
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

const git = (...args) =>
  execFileSync("git", args, { encoding: "utf8", maxBuffer: 1 << 28 });

// What this checkout changes against the merge base with origin/main: commits,
// edits and new files. Empty when it cannot tell, so everything runs.
export function localChanges() {
  try {
    const base = git("merge-base", "origin/main", "HEAD").trim();
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
  const { build, test } = matrices(scope);
  console.log(
    [
      `code=${scope.code}`,
      `rust=${scope.rust}`,
      `platform=${scope.platform}`,
      `rust_build=${JSON.stringify(build)}`,
      `rust_test=${JSON.stringify(test)}`,
    ].join("\n"),
  );
  console.error(
    `${change.files.length} changed file(s): code=${scope.code} rust=${scope.rust} platform=${scope.platform}` +
      (scope.reason ? ` (${scope.reason})` : ""),
  );
}
