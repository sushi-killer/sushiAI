// Runs the Rust workspace tests. With cargo-nextest installed every test binary runs in
// parallel (CI installs it); without it, plain `cargo test` runs them one after another.
import { spawnSync } from "node:child_process";

const run = (args) =>
  spawnSync("cargo", args, { stdio: "inherit" }).status ?? 1;

const hasNextest =
  spawnSync("cargo", ["nextest", "--version"], { stdio: "ignore" }).status ===
  0;
if (!hasNextest) process.exit(run(["test", "--workspace", "--no-fail-fast"]));

// The nextest setup script (.config/nextest.toml) builds the daemon binary once and hands
// it to the tests as SUSHIAI_BIN. nextest does not run doc tests; the workspace has none
// (tests/ci-pipeline.test.cjs keeps it so), so no `cargo test --doc` pass follows.
process.exit(run(["nextest", "run", "--workspace", "--no-fail-fast"]));
