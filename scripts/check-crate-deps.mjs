// Dependency rules for the Rust workspace, checked with `cargo tree`.
// The exit code is the verdict.
//   - sushiai-protocol, sushiai-core, sushiai-hold and sushiai-agents never pull in tokio;
//   - sushiai-agents depends on no other workspace crate;
//   - sushiai-protocol depends on no other workspace crate;
//   - sushiai-core depends on no workspace crate other than sushiai-protocol;
//   - sushiai-hold depends on no workspace crate other than sushiai-protocol.
import { execFileSync } from "node:child_process";

const NO_TOKIO = [
  "sushiai-protocol",
  "sushiai-core",
  "sushiai-hold",
  "sushiai-agents",
];
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

const problems = [];
for (const crate of NO_TOKIO)
  if (dependencies(crate).has("tokio"))
    problems.push(`${crate} depends on tokio`);
for (const [crate, allowed] of Object.entries(ALLOWED_WORKSPACE_DEPS))
  for (const dep of dependencies(crate))
    if (dep.startsWith("sushiai") && !allowed.includes(dep))
      problems.push(`${crate} depends on workspace crate ${dep}`);

if (problems.length) {
  console.error(problems.join("\n"));
  process.exit(1);
}
console.log("crate dependency rules hold");
