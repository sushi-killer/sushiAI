// The data behind the macOS rule of scripts/ci-changes.mjs, kept in
// scripts/ci-platform.json:
// - `crates`: workspace crates whose own Cargo.toml names a platform-only
//   dependency (a `[target.'cfg(...)'.dependencies]` table or a crate from
//   PLATFORM_DEPS). Derived from the manifests.
// - `macosOnly`: tests that failed on the macOS runner while the ubuntu runner
//   of the same run passed them, with the runs that show it (from the CI job
//   logs; see artifacts/PLAN-ci-parallel.md). `files` are the files that
//   define each test, derived from the source.
// `node scripts/ci-platform.mjs` exits 1 when the committed data is stale;
// `--write` rewrites it. tests/ci-pipeline.test.cjs runs the same check.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
export const DATA = path.join(root, "scripts", "ci-platform.json");

// Crates whose job is to bind one operating system's API: a change next to
// them is a change to platform behaviour.
const PLATFORM_DEPS = [
  "portable-pty",
  "nix",
  "rustix",
  "signal-hook",
  "windows-sys",
  "core-foundation",
  "security-framework",
];

function rustFiles(dir, out = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.name === "target" || entry.name.startsWith(".")) continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) rustFiles(full, out);
    else if (entry.name.endsWith(".rs")) out.push(full);
  }
  return out;
}

export function platformCrates(base = root) {
  const crates = path.join(base, "crates");
  return fs
    .readdirSync(crates, { withFileTypes: true })
    .filter((e) => e.isDirectory())
    .map((e) => `crates/${e.name}`)
    .filter((dir) => {
      const manifest = path.join(base, dir, "Cargo.toml");
      if (!fs.existsSync(manifest)) return false;
      const text = fs.readFileSync(manifest, "utf8");
      if (/^\[target\..*dependencies\]/m.test(text)) return true;
      const deps = text
        .split(/^\[/m)
        .filter((section) => /^[\w.-]*dependencies\]/.test(section))
        .flatMap((section) => [...section.matchAll(/^([\w-]+)\s*[.=]/gm)])
        .map((m) => m[1]);
      return deps.some((d) => PLATFORM_DEPS.includes(d));
    })
    .sort();
}

/** The files under crates/ that define test function `name`. */
export function testFiles(name, base = root) {
  const fn = new RegExp(`\\bfn ${name}\\s*\\(`);
  return rustFiles(path.join(base, "crates"))
    .filter((file) => fn.test(fs.readFileSync(file, "utf8")))
    .map((file) => path.relative(base, file).split(path.sep).join("/"))
    .sort();
}

export function generate(current, base = root) {
  return {
    crates: platformCrates(base),
    macosOnly: current.macosOnly.map((entry) => ({
      ...entry,
      files: testFiles(entry.test.split(/[\s:]+/).pop(), base),
    })),
  };
}

export function readData() {
  return JSON.parse(fs.readFileSync(DATA, "utf8"));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const current = readData();
  const fresh = generate(current);
  if (process.argv.includes("--write")) {
    const prettier = await import("prettier");
    fs.writeFileSync(
      DATA,
      await prettier.format(JSON.stringify(fresh), { parser: "json" }),
    );
  } else if (JSON.stringify(fresh) !== JSON.stringify(current)) {
    console.error(
      "scripts/ci-platform.json is stale: run node scripts/ci-platform.mjs --write",
    );
    process.exit(1);
  }
}
