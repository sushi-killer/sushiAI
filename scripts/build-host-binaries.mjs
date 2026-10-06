#!/usr/bin/env node
// Builds the `sushiai` host binary for every platform the SSH installer can meet and writes
// dist/host/<target>/sushiai, dist/host/SHA256SUMS and dist/host/manifest.json.
//
// Usage: node scripts/build-host-binaries.mjs [--targets a,b] [--out dist/host] [--docker-smoke]
//   Targets are keys of TARGETS below (default: all four; darwin targets are skipped on a
//   non-darwin host). Output is built in a temp directory next to --out and renamed into
//   place only when every target succeeded.
//
// Linux targets use `cargo zigbuild` (musl, static). Darwin targets use `cargo build`.
// Missing tools are reported with the install command; nothing is installed here.
// Not part of `npm run ci`: a cold build of all four takes minutes.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const PINNED_ZIG = "0.15.2";

// `unameKey` is what `uname -sm` prints on the host, the installer's lookup key.
const TARGETS = {
  "x86_64-unknown-linux-musl": {
    unameKey: "Linux x86_64",
    zig: true,
    os: "linux",
  },
  "aarch64-unknown-linux-musl": {
    unameKey: "Linux aarch64",
    zig: true,
    os: "linux",
  },
  "aarch64-apple-darwin": {
    unameKey: "Darwin arm64",
    zig: false,
    os: "darwin",
  },
  "x86_64-apple-darwin": {
    unameKey: "Darwin x86_64",
    zig: false,
    os: "darwin",
  },
};

let staging = null;

function fail(message) {
  if (staging) rmSync(staging, { recursive: true, force: true });
  console.error(`build-host-binaries: ${message}`);
  process.exit(1);
}

function run(command, args, options = {}) {
  return spawnSync(command, args, { cwd: ROOT, encoding: "utf8", ...options });
}

const HELP = `Usage: node scripts/build-host-binaries.mjs [--targets a,b] [--out dist/host] [--docker-smoke]

  --targets   comma-separated: ${Object.keys(TARGETS).join(", ")}
              (default: all; darwin targets are skipped on a non-darwin host)
  --out       output directory (default dist/host), replaced only if every target built
  --docker-smoke  also run the linux binaries in an alpine container (--rm)
  --help      this text`;

function parseArgs(argv) {
  const options = {
    targets: Object.keys(TARGETS),
    out: "dist/host",
    dockerSmoke: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--targets")
      options.targets = (argv[++i] ?? "").split(",").filter(Boolean);
    else if (arg === "--out")
      options.out = argv[++i] ?? fail("--out needs a value");
    else if (arg === "--docker-smoke") options.dockerSmoke = true;
    else if (arg === "--help" || arg === "-h") {
      console.log(HELP);
      process.exit(0);
    } else fail(`unknown argument ${arg}`);
  }
  if (options.targets.length === 0) fail("--targets is empty");
  if (new Set(options.targets).size !== options.targets.length) {
    fail(`duplicate target in --targets: ${options.targets.join(",")}`);
  }
  for (const target of options.targets) {
    if (!TARGETS[target])
      fail(
        `unknown target ${target}; known: ${Object.keys(TARGETS).join(", ")}`,
      );
  }
  return options;
}

function preflight(targets) {
  const missing = [];
  if (run("cargo", ["--version"]).status !== 0)
    missing.push("cargo: install rustup from https://rustup.rs");
  const installed = run("rustup", ["target", "list", "--installed"]);
  const have = new Set(
    (installed.stdout ?? "").split("\n").map((l) => l.trim()),
  );
  for (const target of targets) {
    if (!have.has(target))
      missing.push(`target ${target}: rustup target add ${target}`);
  }
  if (targets.some((t) => TARGETS[t].zig)) {
    const zig = run("zig", ["version"]);
    if (zig.status !== 0)
      missing.push(
        `zig ${PINNED_ZIG}: install zig (brew install zig or ziglang.org)`,
      );
    else if (zig.stdout.trim() !== PINNED_ZIG) {
      missing.push(`zig ${PINNED_ZIG} is pinned, found ${zig.stdout.trim()}`);
    }
    if (run("cargo", ["zigbuild", "--help"]).status !== 0) {
      missing.push("cargo-zigbuild: cargo install cargo-zigbuild");
    }
  }
  if (
    targets.some((t) => TARGETS[t].os === "linux") &&
    run("which", ["file"]).status !== 0
  ) {
    missing.push("file: install the file(1) utility");
  }
  if (targets.some((t) => TARGETS[t].os === "darwin")) {
    for (const tool of ["strip", "codesign"]) {
      if (run("which", [tool]).status !== 0)
        missing.push(`${tool}: install the Xcode command line tools`);
    }
  }
  if (missing.length)
    fail(`missing prerequisites:\n  - ${missing.join("\n  - ")}`);
}

function sha256(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

function workspaceVersion() {
  const match = /^version = "([^"]+)"/m.exec(
    readFileSync(join(ROOT, "Cargo.toml"), "utf8"),
  );
  return match ? match[1] : fail("no version in Cargo.toml");
}

function build(target) {
  const args = [TARGETS[target].zig ? "zigbuild" : "build"];
  args.push("--release", "--locked", "-p", "sushiai", "--target", target);
  const started = Date.now();
  const result = run("cargo", args, { stdio: "inherit" });
  if (result.status !== 0) fail(`cargo ${args.join(" ")} failed`);
  return ((Date.now() - started) / 1000).toFixed(1);
}

function finish(target, binary) {
  if (TARGETS[target].os === "darwin") {
    // Stripping rewrites the file and drops the signature; arm64 macOS refuses unsigned code.
    if (run("strip", ["-x", binary]).status !== 0)
      fail(`strip failed for ${target}`);
    if (run("codesign", ["-s", "-", "-f", binary]).status !== 0)
      fail(`codesign failed for ${target}`);
  } else {
    const described = run("file", [binary]).stdout ?? "";
    if (!/statically linked|static-pie linked/.test(described)) {
      fail(`${target} is not statically linked: ${described.trim()}`);
    }
  }
}

function hostKey() {
  return (run("uname", ["-sm"]).stdout ?? "").trim();
}

function smoke(binary) {
  const result = run(binary, ["--version"]);
  if (result.status !== 0 || !/^sushiai \d/.test(result.stdout)) {
    fail(`smoke run failed for ${binary}: ${result.stdout}${result.stderr}`);
  }
  return result.stdout.trim();
}

function dockerSmoke(target, binary) {
  const platform = target.startsWith("x86_64") ? "linux/amd64" : "linux/arm64";
  const result = run("docker", [
    "run",
    "--rm",
    "--platform",
    platform,
    "-v",
    `${binary}:/sushiai:ro`,
    "alpine",
    "/sushiai",
    "--version",
  ]);
  if (result.status !== 0)
    fail(`docker smoke failed for ${target}: ${result.stdout}${result.stderr}`);
  return result.stdout.trim();
}

function targetDirectory() {
  const result = run(
    "cargo",
    ["metadata", "--format-version", "1", "--no-deps"],
    {
      maxBuffer: 256 * 1024 * 1024,
    },
  );
  if (result.status !== 0) fail(`cargo metadata failed: ${result.stderr}`);
  return JSON.parse(result.stdout).target_directory;
}

const options = parseArgs(process.argv.slice(2));
const skipped = options.targets.filter(
  (t) => TARGETS[t].os === "darwin" && process.platform !== "darwin",
);
for (const target of skipped)
  console.log(`skipping ${target}: darwin targets need a macOS host`);
options.targets = options.targets.filter((t) => !skipped.includes(t));
if (options.targets.length === 0) fail("no target left to build on this host");
preflight(options.targets);
const out = resolve(ROOT, options.out);
staging = `${out}.tmp-${process.pid}`;
rmSync(staging, { recursive: true, force: true });
mkdirSync(staging, { recursive: true });
const cargoTarget = targetDirectory();
const version = workspaceVersion();
const manifest = {};
const sums = [];
const host = hostKey();

for (const target of options.targets) {
  const seconds = build(target);
  const dir = join(staging, target);
  mkdirSync(dir, { recursive: true });
  const binary = join(dir, "sushiai");
  copyFileSync(join(cargoTarget, target, "release", "sushiai"), binary);
  chmodSync(binary, 0o755);
  finish(target, binary);

  const { unameKey } = TARGETS[target];
  let smoked = "not run (other architecture)";
  if (unameKey === host) smoked = smoke(binary);
  if (options.dockerSmoke && TARGETS[target].os === "linux")
    smoked = `docker: ${dockerSmoke(target, binary)}`;

  const digest = sha256(binary);
  const size = statSync(binary).size;
  manifest[unameKey] = {
    target,
    path: `${target}/sushiai`,
    version,
    size,
    sha256: digest,
  };
  sums.push(`${digest}  ${target}/sushiai`);
  console.log(
    `${target}: ${(size / 1e6).toFixed(1)} MB, built in ${seconds} s, smoke: ${smoked}`,
  );
}

writeFileSync(join(staging, "SHA256SUMS"), `${sums.join("\n")}\n`);
writeFileSync(
  join(staging, "manifest.json"),
  `${JSON.stringify(manifest, null, 2)}\n`,
);
// Every target built: replace the previous output in one step.
rmSync(out, { recursive: true, force: true });
renameSync(staging, out);
staging = null;
console.log(`wrote ${join(options.out, "manifest.json")} and SHA256SUMS`);
