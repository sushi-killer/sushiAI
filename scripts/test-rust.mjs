// Runs the Rust workspace tests. With cargo-nextest installed every test binary runs in
// parallel (CI installs it); without it, plain `cargo test` runs them one after another.
import { spawnSync } from "node:child_process";
import path from "node:path";

const run = (args, extraEnv = {}) =>
  spawnSync("cargo", args, {
    stdio: "inherit",
    env: { ...process.env, ...extraEnv },
  }).status ?? 1;

const hasNextest =
  spawnSync("cargo", ["nextest", "--version"], { stdio: "ignore" }).status ===
  0;
if (!hasNextest) process.exit(run(["test", "--workspace", "--no-fail-fast"]));

// Each nextest test is its own process: hand the daemon binary over once
// instead of letting every orchestrator test run `cargo build` for it.
if (run(["build", "-p", "sushiai"]) !== 0) process.exit(1);
const target = process.env.CARGO_TARGET_DIR || "target";
const tests = run(["nextest", "run", "--workspace", "--no-fail-fast"], {
  SUSHIAI_BIN: path.resolve(target, "debug", "sushiai"),
});
const docs = run(["test", "--workspace", "--doc"]);
process.exit(tests || docs);
