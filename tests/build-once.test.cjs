const { test, beforeEach, afterEach } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync, spawnSync } = require("node:child_process");

const script = path.resolve(__dirname, "../scripts/build-once.mjs");
let base;
let repo;
let counter;

function git(...args) {
  return execFileSync(
    "git",
    ["-c", "user.name=Tester", "-c", "user.email=t@example.invalid", ...args],
    { cwd: repo, encoding: "utf8" },
  );
}

function fakeBuild(exitCode = 0) {
  const code = [
    "const fs=require('fs');",
    "fs.mkdirSync('dist',{recursive:true});",
    "fs.writeFileSync('dist/index.html','<html></html>');",
    `fs.appendFileSync(${JSON.stringify(counter)},'x\\n');`,
    `process.exit(${exitCode});`,
  ].join("");
  return `${JSON.stringify(process.execPath)} -e ${JSON.stringify(code)}`;
}

function run(args = [], exitCode = 0) {
  return spawnSync(process.execPath, [script, ...args], {
    cwd: repo,
    encoding: "utf8",
    env: { ...process.env, BUILD_ONCE_COMMAND: fakeBuild(exitCode) },
  });
}

const builds = () =>
  fs.existsSync(counter)
    ? fs.readFileSync(counter, "utf8").split("\n").filter(Boolean).length
    : 0;
const stamp = () => path.join(repo, "dist", ".build-once-tree");
const readStamp = () => fs.readFileSync(stamp(), "utf8").trim();

beforeEach(() => {
  base = fs.mkdtempSync("/tmp/sushiai-build-once-test-");
  repo = path.join(base, "repo");
  counter = path.join(base, "counter");
  fs.mkdirSync(repo);
  git("init", "-q", "-b", "main");
  fs.writeFileSync(path.join(repo, ".gitignore"), "dist/\n");
  fs.writeFileSync(path.join(repo, "tracked.txt"), "one\n");
  git("add", "-A");
  git("commit", "-q", "-m", "init");
});

afterEach(() => {
  fs.rmSync(base, { recursive: true, force: true });
});

test("a second run with no change does not build", () => {
  const first = run();
  assert.equal(first.status, 0, first.stderr);
  assert.match(first.stdout, /built tree/);
  assert.equal(builds(), 1);
  const second = run();
  assert.equal(second.status, 0, second.stderr);
  assert.match(second.stdout, /already built/);
  assert.equal(builds(), 1);
});

test("an edited tracked file triggers a build", () => {
  run();
  const before = readStamp();
  fs.writeFileSync(path.join(repo, "tracked.txt"), "two\n");
  const result = run();
  assert.equal(result.status, 0, result.stderr);
  assert.equal(builds(), 2);
  assert.notEqual(readStamp(), before);
});

test("a same-size tracked edit with unchanged stat metadata triggers a build", () => {
  git("config", "core.checkStat", "minimal");
  git("config", "core.trustctime", "false");
  const file = path.join(repo, "tracked.txt");
  const timestamp = new Date("2020-01-01T00:00:00Z");
  fs.utimesSync(file, timestamp, timestamp);
  git("add", "tracked.txt");
  const index = path.resolve(
    repo,
    git("rev-parse", "--git-path", "index").trim(),
  );
  const originalIndex = fs.readFileSync(index);
  const first = run();
  assert.equal(first.status, 0, first.stderr);
  const before = readStamp();

  fs.writeFileSync(file, "two\n");
  fs.utimesSync(file, timestamp, timestamp);
  const result = run();
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /built tree/);
  assert.equal(builds(), 2);
  assert.notEqual(readStamp(), before);
  assert.deepEqual(fs.readFileSync(index), originalIndex);
});

test("an untracked file triggers a build", () => {
  run();
  const before = readStamp();
  fs.writeFileSync(path.join(repo, "new.txt"), "new\n");
  const result = run();
  assert.equal(result.status, 0, result.stderr);
  assert.equal(builds(), 2);
  assert.notEqual(readStamp(), before);
});

test("a deleted stamp triggers a build and is rewritten", () => {
  run();
  const before = readStamp();
  fs.rmSync(stamp());
  const result = run();
  assert.equal(result.status, 0, result.stderr);
  assert.equal(builds(), 2);
  assert.equal(readStamp(), before);
});

test("--stamp writes the stamp without building and the next run skips", () => {
  const stamped = run(["--stamp"]);
  assert.equal(stamped.status, 0, stamped.stderr);
  assert.equal(builds(), 0);
  assert.ok(fs.existsSync(stamp()));
  const next = run();
  assert.match(next.stdout, /already built/);
  assert.equal(builds(), 0);
});

test("the real index is untouched", () => {
  fs.writeFileSync(path.join(repo, "new.txt"), "new\n");
  const result = run();
  assert.equal(result.status, 0, result.stderr);
  assert.match(git("status", "--porcelain"), /^\?\? new\.txt$/m);
  assert.equal(git("diff", "--cached", "--name-only").trim(), "");
});

test("a staged ignored file is hashed without changing its staged content", () => {
  fs.appendFileSync(path.join(repo, ".gitignore"), "ignored-*\n");
  const name = "ignored- space\nfile.txt";
  const file = path.join(repo, name);
  fs.writeFileSync(file, "one\n");
  git("add", "-f", "--", name);
  const first = run();
  assert.equal(first.status, 0, first.stderr);
  const before = readStamp();
  fs.writeFileSync(file, "two\n");
  const result = run();
  assert.equal(result.status, 0, result.stderr);
  assert.equal(builds(), 2);
  assert.notEqual(readStamp(), before);
  assert.equal(git("show", `:${name}`), "one\n");
});

test("a failing build exits non-zero and leaves no stamp", () => {
  const result = run([], 3);
  assert.equal(result.status, 3);
  assert.equal(fs.existsSync(stamp()), false);
});
