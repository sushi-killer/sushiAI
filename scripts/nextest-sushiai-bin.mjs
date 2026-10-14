// nextest setup script (.config/nextest.toml): builds the workspace test targets once, then
// hands the resulting daemon binary to every test through NEXTEST_ENV as SUSHIAI_BIN.
// `cargo build -p sushiai` links the binary with other dependency features than the
// workspace test build does, so building it separately made the second build replace
// the first one's file. This is the same build nextest runs, so it normally finds
// everything up to date and relinks nothing.
import { appendFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";

const cargo = (args) =>
  spawnSync("cargo", args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  });

const built = spawnSync(
  "cargo",
  ["test", "--workspace", "--no-run", "--quiet"],
  { stdio: "inherit" },
);
if (built.status !== 0) process.exit(built.status ?? 1);

const meta = cargo(["metadata", "--format-version", "1", "--no-deps"]);
if (meta.status !== 0) process.exit(meta.status ?? 1);
const target = JSON.parse(meta.stdout).target_directory;
const envFile = process.env.NEXTEST_ENV;
if (!envFile) {
  console.error("NEXTEST_ENV is not set: run this as a nextest setup script");
  process.exit(1);
}
appendFileSync(
  envFile,
  `SUSHIAI_BIN=${path.join(target, "debug", "sushiai")}\n`,
);
