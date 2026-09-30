const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const repoRoot = path.resolve(__dirname, "..");
const sourceScript = path.join(repoRoot, "scripts/new-release-fragment.sh");

function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "new-release-fragment-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const scripts = path.join(root, "scripts");
  const cwd = path.join(root, "elsewhere");
  fs.mkdirSync(scripts);
  fs.mkdirSync(path.join(root, "docs/releases/unreleased"), {
    recursive: true,
  });
  fs.mkdirSync(cwd);
  const script = path.join(scripts, "new-release-fragment.sh");
  fs.copyFileSync(sourceScript, script);
  fs.chmodSync(script, fs.statSync(sourceScript).mode);
  return { root, cwd, script };
}

function run(f, ...args) {
  return spawnSync(f.script, args, { cwd: f.cwd, encoding: "utf8" });
}

test("writes the exact fragment from another working directory", (t) => {
  const f = fixture(t);
  const result = run(f, "demo", "Demo");
  const relativePath = "docs/releases/unreleased/demo.md";
  const content = "## Demo\n\n- TODO: describe the change for users.\n";
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.stdout.split("\n").slice(0, 2), [
    relativePath,
    "Fragments are user-facing prose, in English, with no attribution trailers.",
  ]);
  assert.equal(
    fs.readFileSync(path.join(f.root, relativePath), "utf8"),
    content,
  );

  const prettier = spawnSync(
    path.join(repoRoot, "node_modules/.bin/prettier"),
    ["--check", "--stdin-filepath", "fragment.md"],
    { cwd: repoRoot, encoding: "utf8", input: content },
  );
  assert.equal(prettier.status, 0, prettier.stderr || prettier.stdout);

  const repeated = run(f, "demo", "Demo");
  assert.notEqual(repeated.status, 0);
  assert.match(repeated.stderr, /already exists/);
  assert.equal(
    fs.readFileSync(path.join(f.root, relativePath), "utf8"),
    content,
  );
});

test("rejects invalid arguments without writing a fragment", (t) => {
  const f = fixture(t);
  for (const args of [[], ["demo"], ["demo", ""], ["../x", "Bad"]]) {
    const result = run(f, ...args);
    assert.equal(result.status, 2, `${args}: ${result.stderr}`);
    assert.match(result.stderr, /^Usage:/);
  }
  assert.deepEqual(
    fs.readdirSync(path.join(f.root, "docs/releases/unreleased")),
    [],
  );
});
