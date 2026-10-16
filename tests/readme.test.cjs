// README.md is the first page a visitor sees. These tests keep it free of
// stale release pins and keep its links, anchors and npm scripts real.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const root = path.join(__dirname, "..");
const read = (file) => fs.readFileSync(path.join(root, file), "utf8");
const readme = read("README.md");
const install = read("docs/INSTALL.md");

test("README has no stale release pins or wrong agent claims", () => {
  for (const stale of [
    "0.0.6",
    "0.0.1",
    "Alpha",
    "Gemini CLI",
    "Cursor Agent",
  ]) {
    assert.ok(!readme.includes(stale), `README still contains ${stale}`);
  }
  assert.doesNotMatch(readme, /releases\/(download|tag)\/v\d/);
  assert.match(readme, /releases\/latest/);
});

test("docs/INSTALL.md no longer pins 0.0.6 and agrees with README", () => {
  assert.ok(!install.includes("0.0.6"), "INSTALL still contains 0.0.6");
  assert.match(install, /releases\/latest/);
});

test("README keeps the anchors docs/INSTALL.md links to", () => {
  assert.match(readme, /^## Sessions and remote hosts$/m);
  assert.match(readme, /^## Development$/m);
});

test("README covers the current product areas and links their docs", () => {
  for (const word of ["orchd", "Routines", "Vault", "companion"]) {
    assert.match(readme, new RegExp(word, "i"), `README misses ${word}`);
  }
  for (const doc of [
    "docs/orchestrator-settings.md",
    "docs/routines.md",
    "docs/extensions/companion.md",
  ]) {
    assert.ok(readme.includes(doc), `README does not link ${doc}`);
  }
});

test("every relative link and image in README and INSTALL exists", () => {
  for (const [file, text] of [
    ["README.md", readme],
    ["docs/INSTALL.md", install],
  ]) {
    const base = path.dirname(path.join(root, file));
    const targets = [
      ...text.matchAll(/\]\(([^)\s]+)\)/g),
      ...text.matchAll(/src="([^"]+)"/g),
    ].map((m) => m[1]);
    for (const target of targets) {
      if (/^(https?:|mailto:|#)/.test(target)) continue;
      const clean = target.split("#")[0];
      assert.ok(
        fs.existsSync(path.join(base, clean)),
        `${file} links to missing ${target}`,
      );
    }
  }
});

test("every npm script named in README exists in package.json", () => {
  const scripts = JSON.parse(read("package.json")).scripts;
  const named = [...readme.matchAll(/npm (?:run )?([a-z][\w:-]*)/g)].map(
    (m) => m[1],
  );
  const builtin = new Set(["ci", "install", "start", "test"]);
  for (const name of named) {
    assert.ok(
      scripts[name] || builtin.has(name),
      `README names unknown npm script ${name}`,
    );
  }
});
