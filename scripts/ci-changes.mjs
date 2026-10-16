// Which CI jobs a pull request needs, from the files it changes. Docs alone
// skip the builds and tests; the Rust jobs run only when Rust can be affected.
// Prints `code=` and `rust=` lines for $GITHUB_OUTPUT. Without a usable base
// (a push, a new branch) everything runs.
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

// Markdown that tests or tools read is code: skills, agents, prompts.
const CODE_MARKDOWN =
  /^(\.agents|\.claude|crates|src|electron|scripts|tests)\//;
const RUST = [
  /^crates\//,
  /^Cargo\.(toml|lock)$/,
  /^rust-toolchain\.toml$/,
  /^\.config\//,
  /^\.github\/workflows\//,
  /^scripts\/(test-rust|check-crate-deps|nextest-sushiai-bin)\.mjs$/,
  // The Rust tests read these fixtures and spawn node fake harnesses.
  /^electron\/test-fixtures\/mcp-home\//,
  /^\.nvmrc$/,
];

export function isDoc(file) {
  return (
    (file.endsWith(".md") && !CODE_MARKDOWN.test(file)) ||
    /^docs\/.*\.(md|png|svg)$/.test(file)
  );
}

export function classify(files) {
  if (files.length === 0) return { code: true, rust: true };
  const code = !files.every(isDoc);
  return { code, rust: code && files.some((f) => RUST.some((r) => r.test(f))) };
}

// What this checkout changes against the merge base with origin/main: commits,
// edits and new files. Empty when it cannot tell, so everything runs.
export function localChanges() {
  const git = (...args) =>
    execFileSync("git", args, { encoding: "utf8" }).split("\n").filter(Boolean);
  try {
    const [base] = git("merge-base", "origin/main", "HEAD");
    return [
      ...new Set([
        ...git("diff", "--name-only", "--no-renames", base),
        ...git("ls-files", "--others", "--exclude-standard"),
      ]),
    ];
  } catch {
    return [];
  }
}

function changedFiles(base, head) {
  if (!base || /^0+$/.test(base)) return [];
  try {
    // Both sides of a rename, against the merge base: a moved code file counts.
    return execFileSync(
      "git",
      ["diff", "--name-only", "--no-renames", `${base}...${head}`],
      {
        encoding: "utf8",
      },
    )
      .split("\n")
      .filter(Boolean);
  } catch {
    return [];
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const files = changedFiles(process.env.BASE, process.env.HEAD || "HEAD");
  const { code, rust } = classify(files);
  console.log(`code=${code}\nrust=${rust}`);
  console.error(`${files.length} changed file(s): code=${code} rust=${rust}`);
}
