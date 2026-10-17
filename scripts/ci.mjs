// `npm run ci`: the lanes run side by side. Nothing that relinks target/debug/sushiai
// (the nextest Rust test build) may overlap `npm test`, so the Rust tests close the
// build lane instead of getting a lane of their own.
import { fileURLToPath } from "node:url";
import { classify, describe, localChanges } from "./ci-changes.mjs";
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

// The lanes and commands a change needs, chosen as in CI (scripts/ci-changes.mjs).
// The static checks and the conventions always run.
export function lanesFor(scope) {
  const tests = [];
  if (scope.web)
    tests.push("npm run build", "node scripts/build-once.mjs --stamp");
  const node = scope.nodeTests;
  if (node === null || node.length > 0) {
    tests.push("cargo build -p sushiai");
    tests.push(node === null ? "npm test" : `node --test ${node.join(" ")}`);
  }
  if (scope.rustTests === null) tests.push("node scripts/test-rust.mjs");
  else if (scope.rustTests.length > 0)
    tests.push("node scripts/test-rust.mjs --affected");
  return lanes.flatMap((lane) => {
    if (lane.name === "build and tests")
      return tests.length ? [{ ...lane, commands: tests }] : [];
    if (lane.name === "rust lint") return scope.rust ? [lane] : [];
    return [lane];
  });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  // CI_ALL=1 runs every lane whatever changed.
  const change = localChanges();
  const scope =
    process.env.CI_ALL === "1" ? classify([]) : classify(change.files, change);
  console.log(
    `ci: ${process.env.CI_ALL === "1" ? "CI_ALL=1: everything runs" : describe(scope, change.files.length)}` +
      (scope.all ? "" : " (CI_ALL=1 runs everything)"),
  );
  // The Rust tests here run on this host's OS only. CI adds the other one
  // (macOS) only for platform changes; say so where that is not this host.
  if (scope.platform && process.platform !== "darwin")
    console.log(
      `ci: a platform change (${scope.reason}); the macOS Rust tests run in CI`,
    );
  process.exit(await runLanes(lanesFor(scope)));
}
