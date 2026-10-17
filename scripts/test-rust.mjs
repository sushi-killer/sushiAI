// Runs the Rust workspace tests. With cargo-nextest installed every test binary runs in
// parallel (CI installs it); without it, plain `cargo test` runs them one after another.
// `--affected` runs only the test binaries this checkout's change can affect
// (scripts/ci-changes.mjs); CI_ALL=1 or a missing nextest runs them all.
import { spawnSync } from "node:child_process";
import { classify, describe, localChanges } from "./ci-changes.mjs";
import { nextestFilter } from "./lib/rust-graph.mjs";

const run = (args) =>
  spawnSync("cargo", args, { stdio: "inherit" }).status ?? 1;

const hasNextest =
  spawnSync("cargo", ["nextest", "--version"], { stdio: "ignore" }).status ===
  0;
if (!hasNextest) process.exit(run(["test", "--workspace", "--no-fail-fast"]));

// The nextest setup script (.config/nextest.toml) builds the daemon binary once and hands
// it to the tests as SUSHIAI_BIN. nextest does not run doc tests; the workspace has none
// (tests/ci-pipeline.test.cjs keeps it so), so no `cargo test --doc` pass follows.
const args = ["nextest", "run", "--workspace", "--no-fail-fast"];
if (process.argv.includes("--affected") && process.env.CI_ALL !== "1") {
  const change = localChanges();
  const scope = classify(change.files, change);
  console.log(`test-rust: ${describe(scope, change.files.length)}`);
  if (scope.rustTests?.length === 0) process.exit(0);
  if (scope.rustTests) args.push("-E", nextestFilter(scope.rustTests));
}
process.exit(run(args));
