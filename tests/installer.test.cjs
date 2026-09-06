const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { createHash } = require("node:crypto");
const {
  prepare,
  replace,
  cleanupCompleted,
} = require("../electron/install-helper.cjs");
async function fixture(t) {
  const directory = await fs.realpath(
    await fs.mkdtemp("/tmp/sushiai-installer-test-"),
  );
  const target = path.join(directory, "sushiAI.app");
  await fs.mkdir(target);
  await fs.writeFile(path.join(target, "version"), "old");
  const dmg = path.join(directory, "update.dmg");
  await fs.writeFile(dmg, "package fixture");
  const plan = {
    target,
    dmg,
    sha256: createHash("sha256").update("package fixture").digest("hex"),
    version: "0.0.2",
    pid: process.pid,
    receipt: path.join(directory, "installed.json"),
    errorFile: path.join(directory, "error.txt"),
  };
  const calls = [];
  async function run(file, args) {
    calls.push([file, args]);
    if (file.endsWith("PlistBuddy"))
      return args[1].includes("Identifier")
        ? "local.sushiai.workspace"
        : "0.0.2";
    if (file.endsWith("hdiutil") && args[0] === "attach") {
      const source = path.join(args[4], "sushiAI.app");
      await fs.mkdir(source);
      await fs.writeFile(path.join(source, "version"), "new");
    }
    if (file.endsWith("hdiutil") && args[0] === "detach")
      await fs.rm(path.join(args[1], "sushiAI.app"), {
        recursive: true,
        force: true,
      });
    if (file.endsWith("ditto"))
      await fs.cp(args[0], args[1], { recursive: true });
    return "";
  }
  t.after(() => fs.rm(directory, { recursive: true, force: true }));
  return { plan, run, calls, directory };
}
test("installer stages and verifies before replacing, then retains backup until healthy startup", async (t) => {
  const { plan, run, calls } = await fixture(t);
  const staged = await prepare(plan, { run });
  assert.equal(
    await fs.readFile(path.join(plan.target, "version"), "utf8"),
    "old",
  );
  assert.equal(calls.filter(([file]) => file.endsWith("codesign")).length, 3);
  let aliveChecks = 0;
  await replace(plan, staged, {
    run,
    isAlive: () => ++aliveChecks < 3,
    sleep: async () => {},
  });
  assert.equal(aliveChecks, 3);
  assert.equal(
    await fs.readFile(path.join(plan.target, "version"), "utf8"),
    "new",
  );
  assert.equal(
    await fs.readFile(path.join(staged.backup, "version"), "utf8"),
    "old",
  );
  assert.ok(
    calls.some(
      ([file, args]) => file.endsWith("open") && args[1] === plan.target,
    ),
  );
  await cleanupCompleted(plan.receipt, plan.target, "0.0.1");
  assert.ok(await fs.stat(staged.backup));
  await cleanupCompleted(plan.receipt, plan.target, "0.0.2");
  await assert.rejects(fs.stat(staged.directory), { code: "ENOENT" });
});
test("bad checksum or wrong bundle version never changes the installed app", async (t) => {
  const { plan, run, directory } = await fixture(t);
  await assert.rejects(
    prepare({ ...plan, sha256: "0".repeat(64) }, { run }),
    /SHA-256/,
  );
  await assert.rejects(
    prepare({ ...plan, version: "9.0.0" }, { run }),
    /version/,
  );
  assert.equal(
    await fs.readFile(path.join(plan.target, "version"), "utf8"),
    "old",
  );
  assert.equal(
    (await fs.readdir(directory)).some((name) =>
      name.startsWith(".sushiAI-update-"),
    ),
    false,
  );
});
test("failed replacement restores the previous app", async (t) => {
  const { plan, run } = await fixture(t);
  const staged = await prepare(plan, { run });
  await assert.rejects(
    replace(plan, staged, {
      run,
      isAlive: () => false,
      rename: async (a, b) => {
        if (a === staged.staged) throw Error("Disk failure");
        await fs.rename(a, b);
      },
    }),
    /Disk failure/,
  );
  assert.equal(
    await fs.readFile(path.join(plan.target, "version"), "utf8"),
    "old",
  );
});
test("failed launch rolls back and a running old process prevents replacement", async (t) => {
  const { plan, run } = await fixture(t);
  let staged = await prepare(plan, { run });
  await assert.rejects(
    replace(plan, staged, { run, isAlive: () => true, sleep: async () => {} }),
    /did not exit/,
  );
  assert.equal(
    await fs.readFile(path.join(plan.target, "version"), "utf8"),
    "old",
  );
  staged = await prepare(plan, { run });
  let launches = 0;
  await assert.rejects(
    replace(plan, staged, {
      isAlive: () => false,
      run: async (file, args) => {
        if (file.endsWith("open") && ++launches === 1)
          throw Error("Launch failed");
        return run(file, args);
      },
    }),
    /Launch failed/,
  );
  assert.equal(
    await fs.readFile(path.join(plan.target, "version"), "utf8"),
    "old",
  );
  assert.equal(launches, 2);
});
