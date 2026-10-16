// `npm run ci`: the lanes run side by side. Nothing that relinks target/debug/sushiai
// (the nextest Rust test build) may overlap `npm test`, so the Rust tests close the
// build lane instead of getting a lane of their own.
import { fileURLToPath } from "node:url";
import { classify, localChanges } from "./ci-changes.mjs";
import { runLanes } from "./lib/run-lanes.mjs";

export const lanes = [
  {
    name: "build and tests",
    commands: [
      "npm run build",
      "node scripts/build-once.mjs --stamp",
      "cargo build -p sushiai",
      "npm test",
      "node scripts/test-rust.mjs",
    ],
  },
  {
    name: "static",
    commands: [
      "npm run lint",
      "./node_modules/.bin/prettier --check src electron scripts tests *.json *.ts *.html README.md",
      "git diff --check",
    ],
  },
  { name: "conventions", commands: ["npm run check:conventions"] },
  { name: "rust lint", commands: ["npm run lint:rust"] },
];

// The lanes a change needs, as in CI (scripts/ci-changes.mjs): docs alone run
// the static checks, and the Rust tests and lint run only when Rust can be affected.
export function lanesFor({ code, rust }) {
  if (!code)
    return lanes.filter((l) => ["static", "conventions"].includes(l.name));
  if (rust) return lanes;
  return lanes
    .filter((l) => l.name !== "rust lint")
    .map((l) => ({
      ...l,
      commands: l.commands.filter((c) => c !== "node scripts/test-rust.mjs"),
    }));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  // CI_ALL=1 runs every lane whatever changed.
  const change = localChanges();
  const scope =
    process.env.CI_ALL === "1"
      ? { code: true, rust: true, platform: true }
      : classify(change.files, change);
  const chosen = lanesFor(scope);
  const skipped = lanes.length - chosen.length;
  console.log(
    `ci: code=${scope.code} rust=${scope.rust} platform=${scope.platform}` +
      (skipped || !scope.rust ? " (CI_ALL=1 runs everything)" : ""),
  );
  // The Rust tests here run on this host's OS only. CI adds the other one
  // (macOS) only for platform changes; say so where that is not this host.
  if (scope.platform && process.platform !== "darwin")
    console.log(
      `ci: a platform change (${scope.reason}); the macOS Rust tests run in CI`,
    );
  process.exit(await runLanes(chosen));
}
