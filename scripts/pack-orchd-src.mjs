// Packs the orchd source the release build needs (Cargo manifest, lockfile and
// src/, no target/ and no tests) into build/orchd-src.tar.gz. The packaged app
// ships it as a resource so a remote host of any OS/CPU can build orchd there.
import { spawnSync } from "node:child_process";
import { mkdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const out = path.join(root, "build", "orchd-src.tar.gz");
mkdirSync(path.dirname(out), { recursive: true });
const result = spawnSync(
  "tar",
  [
    "-czf",
    out,
    "-C",
    root,
    "crates/sushiai-orch/Cargo.toml",
    "crates/sushiai-orch/src",
    // `include_str!` targets outside src/ (tests/orchd-src-pack.test.cjs).
    "crates/sushiai-orch/prompts",
    "crates/sushiai-orch/skills",
  ],
  { stdio: "inherit", env: { ...process.env, COPYFILE_DISABLE: "1" } },
);
if (result.status !== 0) {
  console.error("Could not create build/orchd-src.tar.gz");
  process.exit(result.status ?? 1);
}
console.log(`${out} (${statSync(out).size} bytes)`);
