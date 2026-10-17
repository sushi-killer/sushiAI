// The Rust half of the check selector (scripts/ci-changes.mjs), read from the
// workspace itself: crate dependencies from each crates/*/Cargo.toml, files a
// crate includes from outside its folder through #[path], and the test
// binaries with the crates they depend on. One mapping file,
// scripts/ci-test-map.json, adds what the manifests cannot say: which crates
// the `sushiai` binary's own integration tests exercise, and which crates a
// test that starts that binary also runs (the host).
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "../..");
export const TEST_MAP = path.join(root, "scripts", "ci-test-map.json");
// The binary every daemon-driving test starts.
const BIN = "sushiai";

const posix = (p) => p.split(path.sep).join("/");

function rustFiles(dir, out = []) {
  if (!fs.existsSync(dir)) return out;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.name === "target" || entry.name.startsWith(".")) continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) rustFiles(full, out);
    else if (entry.name.endsWith(".rs")) out.push(full);
  }
  return out;
}

/** Workspace crates: folder, package name and the member crates each depends on. */
export function crates(base = root) {
  const dir = path.join(base, "crates");
  const found = fs
    .readdirSync(dir, { withFileTypes: true })
    .filter(
      (e) =>
        e.isDirectory() && fs.existsSync(path.join(dir, e.name, "Cargo.toml")),
    )
    .map((e) => {
      const text = fs.readFileSync(
        path.join(dir, e.name, "Cargo.toml"),
        "utf8",
      );
      const name =
        /^\[package\][^[]*?^name\s*=\s*"([^"]+)"/ms.exec(text)?.[1] ?? e.name;
      const deps = text
        .split(/^\[/m)
        .filter((s) => /^[\w.'"()-]*dependencies\]/.test(s))
        .flatMap((s) => [...s.matchAll(/^([\w-]+)\s*[.=]/gm)].map((m) => m[1]));
      return { dir: `crates/${e.name}`, name, deps };
    });
  const names = new Set(found.map((c) => c.name));
  for (const c of found)
    c.deps = [...new Set(c.deps.filter((d) => names.has(d) && d !== c.name))];
  return found;
}

/** `name` and every member crate it depends on, directly or not. */
export function forward(all, name, seen = new Set()) {
  if (seen.has(name)) return seen;
  seen.add(name);
  for (const d of all.find((c) => c.name === name)?.deps ?? [])
    forward(all, d, seen);
  return seen;
}

/** Files outside any crate folder, with the crates that include them by #[path]. */
export function sharedFiles(all, base = root) {
  const shared = new Map();
  for (const c of all)
    for (const file of rustFiles(path.join(base, c.dir))) {
      const text = fs.readFileSync(file, "utf8");
      for (const [, rel] of text.matchAll(/#\[path\s*=\s*"([^"]+)"\]/g)) {
        const target = posix(
          path.relative(base, path.resolve(path.dirname(file), rel)),
        );
        if (target.startsWith(`${c.dir}/`)) continue;
        if (!shared.has(target)) shared.set(target, new Set());
        shared.get(target).add(c.name);
      }
    }
  return shared;
}

/** Whether a crate's integration tests start the `sushiai` binary. */
function testsRunBin(base, dir) {
  return rustFiles(path.join(base, dir, "tests")).some((f) =>
    /CARGO_BIN_EXE_sushiai|SUSHIAI_BIN|sushiai_bin\(\)/.test(
      fs.readFileSync(f, "utf8"),
    ),
  );
}

export function readTestMap() {
  return JSON.parse(fs.readFileSync(TEST_MAP, "utf8"));
}

/** The crates a running `sushiai` daemon executes whatever module a test drives. */
export function hostCrates(base = root, map = readTestMap()) {
  const all = crates(base);
  return new Set([BIN, ...map.host.flatMap((h) => [...forward(all, h)])]);
}

/**
 * Every test binary with its nextest id and the crates whose change can
 * break it. A test that starts the binary also depends on the host crates;
 * a `sushiai` integration test file with no mapping depends on every crate.
 */
export function testBinaries(base = root, map = readTestMap()) {
  const all = crates(base);
  const every = new Set(all.map((c) => c.name));
  const host = hostCrates(base, map);
  const out = [];
  for (const c of all) {
    const own = forward(all, c.name);
    const runsBin = testsRunBin(base, c.dir);
    const lib = fs.existsSync(path.join(base, c.dir, "src", "lib.rs"));
    if (lib) out.push({ id: c.name, crate: c.name, deps: own });
    if (fs.existsSync(path.join(base, c.dir, "src", "main.rs")))
      out.push({ id: `${c.name}::bin/${c.name}`, crate: c.name, deps: own });
    const testsDir = path.join(base, c.dir, "tests");
    const files = fs.existsSync(testsDir)
      ? fs.readdirSync(testsDir).filter((f) => f.endsWith(".rs"))
      : [];
    for (const f of files) {
      const stem = f.replace(/\.rs$/, "");
      let deps = new Set(own);
      if (c.name === BIN) {
        const mapped = map.binTests[stem];
        deps = mapped
          ? new Set([...host, ...mapped.flatMap((m) => [...forward(all, m)])])
          : every;
      } else if (runsBin) deps = new Set([...own, ...host]);
      out.push({ id: `${c.name}::${stem}`, crate: c.name, deps });
    }
  }
  return out;
}

/**
 * The Rust part of a change: `all` when the graph cannot place a file,
 * otherwise the changed crates and the test binaries they select.
 */
export function rustSelection(files, base = root) {
  const all = crates(base);
  const shared = sharedFiles(all, base);
  const changed = new Set();
  for (const file of files) {
    const owner = all.find((c) => file.startsWith(`${c.dir}/`));
    if (owner) {
      // The binary composes every module: a change in it can break any test.
      if (owner.name === BIN)
        return { all: true, reason: `${file}: the ${BIN} binary` };
      changed.add(owner.name);
    } else if (shared.has(file)) {
      for (const name of shared.get(file)) changed.add(name);
    } else if (file.startsWith("crates/")) {
      return { all: true, unknown: true, reason: `${file}: not in any crate` };
    }
  }
  const binaries = testBinaries(base).filter((b) =>
    [...b.deps].some((d) => changed.has(d)),
  );
  return {
    all: false,
    crates: [...changed].sort(),
    binaries: binaries.map((b) => b.id).sort(),
  };
}

/** A nextest filterset that runs exactly `ids`. */
export function nextestFilter(ids) {
  return ids.map((id) => `binary_id(=${id})`).join(" | ");
}
