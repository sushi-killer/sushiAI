const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const { mkdir, mkdtemp, rm, symlink, writeFile } = require("node:fs/promises");
const { tmpdir } = require("node:os");
const path = require("node:path");

const {
  scanLocalExtensions,
  MAX_MANIFEST_BYTES,
} = require("../electron/extensions/local-extensions.cjs");

const probeFixture = JSON.parse(
  fs.readFileSync(
    path.join(__dirname, "fixtures/extensions/probe/manifest.json"),
    "utf8",
  ),
);

const manifest = (id, overrides = {}) => ({
  ...structuredClone(probeFixture),
  id,
  ...overrides,
});

async function folder(root, name, contents) {
  const dir = path.join(root, name);
  await mkdir(dir, { recursive: true });
  if (contents !== undefined)
    await writeFile(
      path.join(dir, "manifest.json"),
      typeof contents === "string" ? contents : JSON.stringify(contents),
    );
  return dir;
}

async function fixture(t) {
  const base = await mkdtemp(path.join(tmpdir(), "sushiai-local-ext-"));
  t.after(() => rm(base, { recursive: true, force: true }));
  return base;
}

test("a valid folder loads with the path stamped by the scanner", async (t) => {
  const base = await fixture(t);
  const root = path.join(base, "extensions");
  const dir = await folder(root, "tasks", manifest("user.specimen"));
  const { manifests, problems } = await scanLocalExtensions(root);
  assert.deepEqual(problems, []);
  assert.equal(manifests.size, 1);
  const found = manifests.get("user.specimen");
  assert.equal(found.source.kind, "local");
  assert.equal(
    found.source.path,
    await require("node:fs/promises").realpath(dir),
  );
  assert.equal(
    found.contributions.surfaces[0].extensionId,
    "user.specimen",
    "contributions stay namespaced to their extension",
  );
});

test("one broken manifest costs only its own folder", async (t) => {
  const base = await fixture(t);
  const root = path.join(base, "extensions");
  await folder(root, "good", manifest("user.good"));
  await folder(root, "broken", "{ not json");
  const { manifests, problems } = await scanLocalExtensions(root);
  assert.deepEqual([...manifests.keys()], ["user.good"]);
  assert.equal(problems.length, 1);
  assert.equal(problems[0].folder, "broken");
  assert.match(problems[0].error, /manifest\.json/);
});

test("a folder without a manifest is reported, not skipped silently", async (t) => {
  const base = await fixture(t);
  const root = path.join(base, "extensions");
  await folder(root, "empty");
  const { manifests, problems } = await scanLocalExtensions(root);
  assert.equal(manifests.size, 0);
  assert.equal(problems.length, 1);
  assert.match(problems[0].error, /not found/);
});

test("a folder cannot claim where it came from", async (t) => {
  const base = await fixture(t);
  const root = path.join(base, "extensions");
  await folder(
    root,
    "liar",
    manifest("user.liar", { source: { kind: "builtin" } }),
  );
  await folder(
    root,
    "liar-npm",
    manifest("user.liar-npm", {
      source: { kind: "npm", package: "x", version: "1.0.0" },
    }),
  );
  const { manifests, problems } = await scanLocalExtensions(root);
  assert.equal(manifests.size, 0, "neither folder loads");
  assert.equal(problems.length, 2);
  for (const problem of problems)
    assert.match(problem.error, /must not declare a source/);
});

test("an explicit local source is accepted and re-stamped", async (t) => {
  const base = await fixture(t);
  const root = path.join(base, "extensions");
  await folder(
    root,
    "tasks",
    manifest("user.specimen", { source: { kind: "local" } }),
  );
  const { manifests, problems } = await scanLocalExtensions(root);
  assert.deepEqual(problems, []);
  assert.ok(path.isAbsolute(manifests.get("user.specimen").source.path));
});

test("an oversized manifest is refused before it is parsed", async (t) => {
  const base = await fixture(t);
  const root = path.join(base, "extensions");
  const big = manifest("user.big");
  big.name = "x".repeat(MAX_MANIFEST_BYTES + 10);
  await folder(root, "big", big);
  const { manifests, problems } = await scanLocalExtensions(root);
  assert.equal(manifests.size, 0);
  assert.match(problems[0].error, /larger than/);
});

test("a symlinked checkout outside the folder still loads", async (t) => {
  const base = await fixture(t);
  const root = path.join(base, "extensions");
  await mkdir(root, { recursive: true });
  const elsewhere = await folder(base, "checkout", manifest("user.linked"));
  await symlink(elsewhere, path.join(root, "linked"));
  const { manifests, problems } = await scanLocalExtensions(root);
  assert.deepEqual(problems, [], "developing against a checkout is supported");
  assert.equal(manifests.size, 1);
  assert.match(manifests.get("user.linked").source.path, /checkout$/);
});

test("dot folders are ignored", async (t) => {
  const base = await fixture(t);
  const root = path.join(base, "extensions");
  await folder(root, ".hidden", manifest("user.hidden"));
  await folder(root, "visible", manifest("user.visible"));
  const { manifests, problems } = await scanLocalExtensions(root);
  assert.deepEqual([...manifests.keys()], ["user.visible"]);
  assert.deepEqual(problems, []);
});

test("two folders claiming one id load neither", async (t) => {
  const base = await fixture(t);
  const root = path.join(base, "extensions");
  await folder(root, "a-first", manifest("user.same"));
  await folder(root, "b-second", manifest("user.same"));
  const { manifests, problems } = await scanLocalExtensions(root);
  assert.equal(manifests.size, 0, "the outcome does not depend on read order");
  assert.equal(problems.length, 2);
  assert.deepEqual(
    problems.map((problem) => problem.folder).sort(),
    ["a-first", "b-second"],
  );
});

test("a folder cannot shadow a bundled extension", async (t) => {
  const base = await fixture(t);
  const root = path.join(base, "extensions");
  await folder(root, "fake-herdr", manifest("builtin.herdr"));
  const { manifests, problems } = await scanLocalExtensions(root, [
    "builtin.herdr",
  ]);
  assert.equal(manifests.size, 0);
  assert.match(problems[0].error, /already in use/);
});

test("an unreadable root is reported instead of throwing", async (t) => {
  const base = await fixture(t);
  // A plain file where the folder is expected: mkdir and readdir both fail.
  const root = path.join(base, "not-a-folder");
  await writeFile(root, "");
  const { manifests, problems } = await scanLocalExtensions(root);
  assert.equal(manifests.size, 0);
  assert.equal(problems.length, 1);
  assert.match(problems[0].error, /unreadable/i);
});

test("a manifest that asks for a core view is refused", async (t) => {
  const base = await fixture(t);
  const root = path.join(base, "extensions");
  const sneaky = manifest("user.sneaky");
  sneaky.contributions.surfaces[0].view = { kind: "core", viewId: "workspace" };
  await folder(root, "sneaky", sneaky);
  const { manifests, problems } = await scanLocalExtensions(root);
  assert.equal(manifests.size, 0, "only built-ins may request core views");
  assert.match(problems[0].error, /core views/i);
});
