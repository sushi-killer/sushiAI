// Dependency rules for the Rust workspace, checked with `cargo tree`.
// The exit code is the verdict.
//   - sushiai-protocol, sushiai-core, sushiai-hold and sushiai-agents never pull in tokio;
//   - sushiai-agents and sushiai-protocol depend on no other workspace crate;
//   - sushiai-core and sushiai-hold depend on no workspace crate other than sushiai-protocol;
//   - a core crate depends only on other core crates, never on a module crate;
//   - a module crate (any workspace crate that is neither core nor the composition bin) depends
//     on no core crate;
//   - only the composition bin may depend on both. No module is named here.
import { execFileSync } from "node:child_process";

const NO_TOKIO = [
  "sushiai-protocol",
  "sushiai-core",
  "sushiai-hold",
  "sushiai-agents",
];
const CORE = [
  "sushiai-protocol",
  "sushiai-core",
  "sushiai-daemon",
  "sushiai-hold",
  "sushiai-agents",
];
const COMPOSITION = "sushiai";
// Tighter limits for the leaf core crates; every other core crate may use any core crate.
const ALLOWED_WORKSPACE_DEPS = {
  "sushiai-protocol": [],
  "sushiai-core": ["sushiai-protocol"],
  "sushiai-hold": ["sushiai-protocol"],
  "sushiai-agents": [],
};

function dependencies(crate) {
  const out = execFileSync(
    "cargo",
    ["tree", "-e", "normal", "-p", crate, "--prefix", "none"],
    { encoding: "utf8" },
  );
  const names = out
    .split("\n")
    .map((line) => line.split(" ")[0])
    .filter(Boolean);
  // The first line is the crate itself.
  return new Set(names.slice(1));
}

const members = new Set(
  JSON.parse(
    execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
    }),
  ).packages.map((p) => p.name),
);
const modules = [...members].filter(
  (name) => !CORE.includes(name) && name !== COMPOSITION,
);

const problems = [];
for (const crate of NO_TOKIO)
  if (dependencies(crate).has("tokio"))
    problems.push(`${crate} depends on tokio`);
for (const crate of CORE) {
  const allowed = ALLOWED_WORKSPACE_DEPS[crate] ?? CORE;
  for (const dep of dependencies(crate))
    if (members.has(dep) && !allowed.includes(dep))
      problems.push(`${crate} depends on workspace crate ${dep}`);
}
for (const crate of modules)
  for (const dep of dependencies(crate))
    if (CORE.includes(dep))
      problems.push(`module crate ${crate} depends on core crate ${dep}`);

if (problems.length) {
  console.error(problems.join("\n"));
  process.exit(1);
}
console.log("crate dependency rules hold");
