// The Rust half of the check selector (scripts/ci-changes.mjs), read from the
// workspace itself: crate dependencies from each crates/*/Cargo.toml, files a
// crate includes from outside its folder through #[path], and the test
// binaries with the crates they depend on. What the manifests cannot say
// comes from two places: scripts/ci-test-map.json names the core crates a
// test that starts the `sushiai` binary also runs (the host), and each test
// of that binary names the modules it drives in a `ci-crates:` header.
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

/** Files outside any crate folder, with the crates that include them by #[path],
 * include_str! or include_bytes!. */
export function sharedFiles(all, base = root) {
  const shared = new Map();
  for (const c of all)
    for (const file of rustFiles(path.join(base, c.dir))) {
      const text = fs.readFileSync(file, "utf8");
      for (const [, rel] of text
        .matchAll(
          /#\[path\s*=\s*"([^"]+)"\]|include_(?:str|bytes)!\(\s*"([^"]+)"/g,
        )
        .map((m) => [m[0], m[1] ?? m[2]])) {
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

// A test of the `sushiai` binary names what it exercises in a header line,
// at the composition point and not in this script:
//   //! ci-crates: <crate> ...          the module crates it drives
//   //! ci-crates: host                 only the host crates
//   //! ci-crates: crates-with:<file>   every crate whose folder holds <file>
// The host crates are always implied. A test without the line depends on
// every crate (tests/ci-pipeline.test.cjs requires the line).
export const CI_CRATES = /^\/\/! ci-crates:(.*)$/m;

/** The crates a `ci-crates:` header names, or null when the file has none. */
export function headerCrates(text, all, base = root) {
  const line = CI_CRATES.exec(text);
  if (!line) return null;
  return line[1]
    .trim()
    .split(/\s+/)
    .filter((w) => w && w !== "host")
    .flatMap((w) => {
      if (!w.startsWith("crates-with:")) return [w];
      const file = w.slice("crates-with:".length);
      return all
        .filter((c) => fs.existsSync(path.join(base, c.dir, file)))
        .map((c) => c.name);
    });
}

// The names of a manifest's [[bin]] or [[test]] tables.
function tableNames(manifest, kind) {
  return manifest
    .split(/^\[/m)
    .filter((s) => s.startsWith(`[${kind}]]`))
    .map((s) => /^name\s*=\s*"([^"]+)"/m.exec(s)?.[1])
    .filter(Boolean);
}

/** A crate's test targets: [nextest id suffix, integration test source or null]. */
function crateTargets(base, c) {
  const dir = path.join(base, c.dir);
  const manifest = fs.readFileSync(path.join(dir, "Cargo.toml"), "utf8");
  const out = [];
  if (fs.existsSync(path.join(dir, "src", "lib.rs"))) out.push(["", null]);
  const bins = new Set(tableNames(manifest, "bin"));
  if (!bins.size && fs.existsSync(path.join(dir, "src", "main.rs")))
    bins.add(c.name);
  const binDir = path.join(dir, "src", "bin");
  if (fs.existsSync(binDir))
    for (const f of fs.readdirSync(binDir)) bins.add(f.replace(/\.rs$/, ""));
  for (const b of bins) out.push([`::bin/${b}`, null]);
  const tests = new Map(tableNames(manifest, "test").map((t) => [t, null]));
  const testsDir = path.join(dir, "tests");
  if (fs.existsSync(testsDir))
    for (const e of fs.readdirSync(testsDir, { withFileTypes: true })) {
      const main = path.join(testsDir, e.name, "main.rs");
      if (e.isFile() && e.name.endsWith(".rs"))
        tests.set(e.name.slice(0, -3), path.join(testsDir, e.name));
      else if (e.isDirectory() && fs.existsSync(main)) tests.set(e.name, main);
    }
  for (const [t, file] of tests) out.push([`::${t}`, file]);
  return out;
}

/**
 * Every test binary with its nextest id and the crates whose change can
 * break it. A test that starts the binary also depends on the host crates;
 * a `sushiai` integration test without a `ci-crates:` header depends on
 * every crate.
 */
export function testBinaries(base = root, map = readTestMap()) {
  const all = crates(base);
  const every = new Set(all.map((c) => c.name));
  const host = hostCrates(base, map);
  const out = [];
  for (const c of all) {
    const own = forward(all, c.name);
    const runsBin = testsRunBin(base, c.dir);
    for (const [suffix, file] of crateTargets(base, c)) {
      let deps = new Set(own);
      if (file && c.name === BIN) {
        const named = headerCrates(fs.readFileSync(file, "utf8"), all, base);
        deps = named
          ? new Set([...host, ...named.flatMap((m) => [...forward(all, m)])])
          : every;
      } else if (file && runsBin) deps = new Set([...own, ...host]);
      out.push({ id: `${c.name}${suffix}`, crate: c.name, deps, file });
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
