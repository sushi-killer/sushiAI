// Builds sushiAI and installs it as the one copy macOS sees: /Applications/sushiAI.app.
// macOS keeps one privacy grant per bundle id and one signature to trust for it; a second
// copy with another signature (a build left in release/) makes every grant flip between
// them. So this script builds outside the repo's release/ folder, installs, and removes
// the build. It signs with "sushiAI Local Signing" (scripts/setup-local-signing.sh) so the
// grant survives rebuilds; without that identity it stops instead of signing ad hoc.
import { spawnSync } from "node:child_process";
import { existsSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const identity = "sushiAI Local Signing";
const out = path.join(root, "release-next");
const built = path.join(out, "mac-arm64", "sushiAI.app");
const target = "/Applications/sushiAI.app";

const fail = (message) => {
  console.error(`install:app FAILED: ${message}`);
  process.exit(1);
};
const run = (cmd, args, opts = {}) => {
  console.log(`> ${cmd} ${args.join(" ")}`);
  const r = spawnSync(cmd, args, { cwd: root, stdio: "inherit", ...opts });
  if (r.status !== 0) fail(`${cmd} exited with ${r.status}`);
};
const read = (cmd, args) => {
  const r = spawnSync(cmd, args, { encoding: "utf8" });
  return `${r.stdout}${r.stderr}`;
};

if (
  !read("security", ["find-identity", "-v", "-p", "codesigning"]).includes(
    identity,
  )
)
  fail(
    `no "${identity}" signing identity: run scripts/setup-local-signing.sh once.`,
  );

run("npm", ["run", "build:daemon"]);
run("npm", ["run", "build:host"]);
run("npm", ["run", "build"]);
rmSync(out, { recursive: true, force: true });
run("./node_modules/.bin/electron-builder", [
  "--mac",
  "dir",
  `-c.directories.output=${out}`,
  `-c.mac.identity=${identity}`,
]);
run("node", ["scripts/verify-host-manifest.mjs", built]);
// A designated requirement that names a cdhash changes with every build: refuse it.
for (const file of [built, path.join(built, "Contents/Resources/sushiai")]) {
  const dr = read("codesign", ["-dr", "-", file]);
  if (!dr.includes("certificate leaf") || dr.includes("cdhash"))
    fail(`${file} is not signed with a stable requirement:\n${dr}`);
}

// Built while the app runs; replacing it needs the app closed (sessions keep running).
while (
  spawnSync("pgrep", ["-f", "sushiAI.app/Contents/MacOS/sushiAI"]).status === 0
) {
  console.log("Built. Quit sushiAI to install; waiting...");
  spawnSync("sleep", ["3"]);
}
const staged = `${target}.new`;
rmSync(staged, { recursive: true, force: true });
run("ditto", [built, staged]);
if (existsSync(target)) rmSync(target, { recursive: true, force: true });
run("mv", [staged, target]);
rmSync(out, { recursive: true, force: true });
console.log(
  `install:app ok: ${target} (signed with "${identity}"). Start it with: open ${target}`,
);
