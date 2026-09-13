#!/usr/bin/env node
// Compute the next release from merged PR titles since the last tag (each
// merge commit's message equals its PR title - squash-only, see AGENTS.md),
// bump package.json, fold docs/releases/unreleased/*.md fragments into
// docs/releases/<version>.md, and commit it all on a release/<version>
// branch. Run manually, or via the cut-release skill - never from CI.
// No push, no tag: those happen when the resulting PR gets squash-merged
// (see .github/workflows/release.yml).
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync, readdirSync, unlinkSync } from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { computeBump, bumpVersion } from "./release-bump.mjs";

const { CONTRACT } = createRequire(import.meta.url)(
  "../electron/extensions/manifest.cjs",
);

function git(args) {
  return execFileSync("git", args, { encoding: "utf8" }).trim();
}

const override = process.argv[2];
if (override && !["patch", "minor", "major"].includes(override)) {
  console.error(
    `Unknown bump override "${override}" - use patch, minor, or major.`,
  );
  process.exit(1);
}

const status = git(["status", "--porcelain"]);
if (status) {
  console.error("Working tree isn't clean - commit or stash first.");
  process.exit(1);
}

const currentBranch = git(["rev-parse", "--abbrev-ref", "HEAD"]);
if (currentBranch !== "main") {
  console.error(`Must run from main, currently on "${currentBranch}".`);
  process.exit(1);
}

execFileSync("git", ["fetch", "origin", "main", "--quiet"]);
if (git(["rev-parse", "HEAD"]) !== git(["rev-parse", "origin/main"])) {
  console.error("Local main isn't in sync with origin/main - pull first.");
  process.exit(1);
}

const lastTag = git(["describe", "--tags", "--abbrev=0"]);
const subjects = git(["log", `${lastTag}..HEAD`, "--format=%s"])
  .split("\n")
  .filter(Boolean);

const pkg = JSON.parse(readFileSync("package.json", "utf8"));
const isPre1 = pkg.version.startsWith("0.");

const bump = override ?? computeBump(subjects, isPre1);

if (!bump) {
  console.error(
    `No feat/fix/breaking commits since ${lastTag} - nothing to release. ` +
      `Pass an explicit bump (patch|minor|major) to override.`,
  );
  process.exit(1);
}

const expectedVersion = bumpVersion(pkg.version, bump);
const branch = `release/${expectedVersion}`;
try {
  git(["show-ref", "--verify", "--quiet", `refs/heads/${branch}`]);
  console.error(
    `Branch ${branch} already exists - delete it or bump manually.`,
  );
  process.exit(1);
} catch {
  // git show-ref exits non-zero when the ref doesn't exist - good, proceed.
}

const unreleasedDir = "docs/releases/unreleased";
const fragments = readdirSync(unreleasedDir)
  .filter((name) => name.endsWith(".md"))
  .sort();

// Create the branch before mutating anything, so a failure partway through
// (npm version, a rerun, a bad fragment) never leaves main's working tree
// bumped and its fragments deleted with nothing committed.
git(["checkout", "-b", branch]);

execFileSync("npm", ["version", bump, "--no-git-tag-version"], {
  stdio: "inherit",
});
const newVersion = JSON.parse(readFileSync("package.json", "utf8")).version;
if (newVersion !== expectedVersion) {
  console.error(
    `npm computed v${newVersion} but v${expectedVersion} was expected - ` +
      `check for a non-standard version string in package.json.`,
  );
  process.exit(1);
}

const apiVersions = CONTRACT.SUPPORTED_API_VERSIONS.join(", ");
const body = fragments.length
  ? fragments
      .map((name) =>
        readFileSync(path.join(unreleasedDir, name), "utf8").trim(),
      )
      .join("\n\n")
  : "_No user-facing changes recorded for this release._";
writeFileSync(
  `docs/releases/${newVersion}.md`,
  `> Extension API: ${apiVersions}\n\n${body}\n`,
);
for (const name of fragments) unlinkSync(path.join(unreleasedDir, name));

git(["add", "package.json", "package-lock.json", "docs/releases"]);
git(["commit", "-m", `chore(release): v${newVersion}`]);

console.log(
  `Prepared ${branch} (${lastTag} -> v${newVersion}, ${bump} bump, ` +
    `${fragments.length} fragment(s) folded in). Not pushed - review, then ` +
    `push and open a PR.`,
);
