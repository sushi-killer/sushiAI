// Repository conventions, enforced in CI and runnable locally:
//   - shipped source and docs stay English;
//   - commit messages carry no tool attribution trailers.
// Terminal Unicode fixtures are the one deliberate exception: they exist to
// prove the terminal renders non-Latin scripts, so their own text must stay.
import { readdir, readFile } from "node:fs/promises";
import { execFile } from "node:child_process";
import path from "node:path";
import { promisify } from "node:util";

const run = promisify(execFile);
const root = process.cwd();
const CYRILLIC = /[Ѐ-ӿ]/;
const SOURCE_DIRS = ["src", "electron"];
const SOURCE_FILES = /\.(ts|tsx|cjs|mjs|js|css|html)$/;
const TRAILERS = [
  /^\s*co-authored-by:/im,
  /generated with \[?claude/i,
  /🤖 generated with/i,
];

const problems = [];

async function* walk(dir) {
  let entries;
  try {
    entries = await readdir(path.join(root, dir), { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    const relative = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === "node_modules" || entry.name.startsWith(".")) continue;
      yield* walk(relative);
    } else if (SOURCE_FILES.test(entry.name)) yield relative;
  }
}

for (const dir of SOURCE_DIRS)
  for await (const file of walk(dir)) {
    const text = await readFile(path.join(root, file), "utf8");
    const line = text.split("\n").findIndex((value) => CYRILLIC.test(value));
    if (line >= 0)
      problems.push(
        `${file}:${line + 1} contains Cyrillic text; shipped source is English only`,
      );
  }

// The design vocabulary lives in tokens.css; src/styles.css is the legacy file
// the boundary moves out of. Anything already converted spends tokens, so a new
// raw colour there is a regression. File-granular on purpose: no false
// positives, and the line only ever moves forward.
const COLOUR = /#[0-9a-fA-F]{3,8}\b/;
const COLOUR_EXEMPT = new Set(["src/styles.css", "src/styles/tokens.css"]);
for await (const file of walk("src/styles")) {
  if (!file.endsWith(".css") || COLOUR_EXEMPT.has(file)) continue;
  const text = await readFile(path.join(root, file), "utf8");
  const line = text.split("\n").findIndex((value) => COLOUR.test(value));
  if (line >= 0)
    problems.push(
      `${file}:${line + 1} uses a raw colour; converted stylesheets spend tokens from styles/tokens.css`,
    );
}

// The shell is a function of `mode` and one open section, and nothing an
// extension declares may change it. Every src/app file except SectionPage.tsx
// draws chrome, not contributed content: if one of them starts reasoning
// about extensions again, a plugin can hide the sidebar the way it used to.
// They may render contributed entries through the slot components and
// nothing else.
const SHELL_DIR = "src/app";
const SHELL_ALLOWED =
  /^\.\.\/extensions\/(ExtensionSlots\.tsx|registry\.ts|routes\.ts|types\.ts)$/;
const SHELL_IMPORT =
  /(?:\bfrom\s+|\brequire\(\s*|\bimport\(\s*)["']([^"']+)["']/g;
const shellFiles = (await readdir(path.join(root, SHELL_DIR)))
  .filter((name) => name.endsWith(".tsx") && name !== "SectionPage.tsx")
  .map((name) => `${SHELL_DIR}/${name}`);
for (const file of shellFiles) {
  const text = await readFile(path.join(root, file), "utf8");
  for (const [, source] of text.matchAll(SHELL_IMPORT))
    if (source.includes("extensions/") && !SHELL_ALLOWED.test(source))
      problems.push(
        `${file} imports ${source}; shell files read the mode and the route id, never extension internals`,
      );
  if (/SurfaceRenderer|activePage|resolveNavigation/.test(text))
    problems.push(
      `${file} resolves an extension surface; only SectionPage draws a contributed page`,
    );
}

// docs/LESSONS.md is meant to be read every session, so a promoted entry
// collapses to a one-line index entry instead of growing the file forever.
// Strip the fenced format-example first, so it doesn't count as an entry.
try {
  const lessonsPath = process.env.LESSONS_PATH_OVERRIDE || "docs/LESSONS.md";
  const lessons = await readFile(path.join(root, lessonsPath), "utf8");
  const stripped = lessons.replace(/```[\s\S]*?```/g, "");
  const wordCount = (text) => text.trim().split(/\s+/).filter(Boolean).length;
  const total = wordCount(stripped);
  if (total > 600)
    problems.push(
      `docs/LESSONS.md is ${total} words (budget: 600) - promote or drop an open entry`,
    );
  const openSection =
    stripped.split(/^## Open$/m)[1]?.split(/^## Promoted$/m)[0] ?? "";
  const openEntries = openSection.split(/^## .+$/m).slice(1);
  if (openEntries.length > 8)
    problems.push(
      `docs/LESSONS.md has ${openEntries.length} open entries (cap: 8) - promote or drop the oldest`,
    );
  openEntries.forEach((body, i) => {
    const words = wordCount(body);
    if (words > 120)
      problems.push(
        `docs/LESSONS.md open entry #${i + 1} is ${words} words (cap: 120) - trim it`,
      );
  });
} catch {
  // No docs/LESSONS.md - nothing to check.
}

const base = process.env.BASE_REF;
const head = process.env.HEAD_REF;
if (base && head) {
  const { stdout } = await run("git", [
    "log",
    "--format=%H%x1f%B%x1e",
    `${base}..${head}`,
  ]);
  for (const entry of stdout.split("\x1e")) {
    const [hash, body] = entry.split("\x1f");
    if (!hash?.trim() || !body) continue;
    for (const trailer of TRAILERS)
      if (trailer.test(body))
        problems.push(
          `commit ${hash.trim().slice(0, 8)} carries an attribution trailer`,
        );
    if (CYRILLIC.test(body))
      problems.push(
        `commit ${hash.trim().slice(0, 8)} has a non-English message`,
      );
  }
}

// Branch prefix is the version-bump signal scripts/release.mjs reads; the PR
// title is the human-reviewed confirmation of that signal, and becomes the
// literal commit message on main once squash-merged. Keep them in agreement.
const BRANCH_RULE = /^(feature|fix|chore|release)\/[a-z0-9._-]+$/;
const TITLE_RULE = /^([a-z]+)(\([^)]+\))?(!)?: .+/;
const headBranch = process.env.HEAD_BRANCH;
const prTitle = process.env.PR_TITLE;
if (headBranch && prTitle) {
  const branchMatch = BRANCH_RULE.test(headBranch);
  if (!branchMatch)
    problems.push(
      `branch "${headBranch}" doesn't match (feature|fix|chore|release)/<slug>`,
    );
  const titleMatch = prTitle.match(TITLE_RULE);
  if (!titleMatch)
    problems.push(
      `PR title "${prTitle}" doesn't start with a conventional-commit type ("feat: ", "fix(scope): ", ...)`,
    );
  if (branchMatch && titleMatch) {
    const prefix = headBranch.split("/")[0];
    const [, type, scope] = titleMatch;
    if (prefix === "feature" && type !== "feat")
      problems.push(
        `branch prefix "feature/" needs a "feat: " PR title, got "${type}: "`,
      );
    if (prefix === "fix" && type !== "fix")
      problems.push(
        `branch prefix "fix/" needs a "fix: " PR title, got "${type}: "`,
      );
    if (prefix === "release" && !(type === "chore" && scope === "(release)"))
      problems.push(
        `branch prefix "release/" needs a "chore(release): " PR title`,
      );
    // prefix === "chore" accepts any type - it's the catch-all prefix.
  }
  // A breaking change with no release note is what actually burns users.
  if (/^[a-z]+(\([^)]+\))?!: /.test(prTitle) && base && head) {
    const { stdout: added } = await run("git", [
      "diff",
      "--name-status",
      "--diff-filter=A",
      `${base}...${head}`,
      "--",
      "docs/releases/unreleased/",
    ]);
    if (!added.split("\n").some((line) => line.endsWith(".md")))
      problems.push(
        `PR title marks a breaking change ("!") but adds no docs/releases/unreleased/*.md fragment`,
      );
  }
}

if (problems.length) {
  console.error("Repository conventions failed:\n");
  for (const problem of problems) console.error(`  - ${problem}`);
  process.exit(1);
}
console.log("Repository conventions: source is English, commits are clean.");
