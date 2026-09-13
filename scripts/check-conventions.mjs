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
// extension declares may change it. These files draw the chrome: if one of
// them starts reasoning about extensions again, a plugin can hide the sidebar
// the way it used to. They may render contributed entries through the slot
// components and nothing else.
const SHELL_FILES = ["src/app/Sidebar.tsx", "src/app/TitleBar.tsx"];
const SHELL_ALLOWED = /^\.\.\/extensions\/(ExtensionSlots\.tsx|registry\.ts|routes\.ts|types\.ts)$/;
for (const file of SHELL_FILES) {
  const text = await readFile(path.join(root, file), "utf8");
  for (const [, source] of text.matchAll(/from "([^"]+)"/g))
    if (source.includes("extensions/") && !SHELL_ALLOWED.test(source))
      problems.push(
        `${file} imports ${source}; shell files read the mode and the route id, never extension internals`,
      );
  if (/SurfaceRenderer|activePage|resolveNavigation/.test(text))
    problems.push(
      `${file} resolves an extension surface; only SectionPage draws a contributed page`,
    );
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

if (problems.length) {
  console.error("Repository conventions failed:\n");
  for (const problem of problems) console.error(`  - ${problem}`);
  process.exit(1);
}
console.log("Repository conventions: source is English, commits are clean.");
