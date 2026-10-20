const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { closeAppDb } = require("../electron/app-db.cjs");
const { Projects } = require("../electron/projects.cjs");
const { untilTurns } = require("./helpers/deadline.cjs");

const safeStorage = {
  isEncryptionAvailable: () => true,
  getSelectedStorageBackend: () => "keychain",
  encryptString: (value) => Buffer.from(`enc:${value}`),
  decryptString: (value) => value.toString().replace(/^enc:/, ""),
};

/** A store with one project holding HAND (a stored value) and BLANK (none). */
async function fixture(t, options = {}) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "env-providers-"));
  t.after(async () => {
    closeAppDb(dir);
    await fs.rm(dir, { recursive: true, force: true });
  });
  const projects = new Projects({ userDataDir: dir, safeStorage, ...options });
  const project = await projects.upsert({ name: "Demo" });
  await projects.updateEnv(project.id, {
    set: [
      { name: "HAND", secret: true, availableTo: ["agent"] },
      { name: "BLANK", secret: false, availableTo: ["agent"] },
    ],
  });
  await projects.setSecret(project.id, "HAND", "hand-value");
  return { projects, project };
}

const provider = (values, calls = []) => ({
  async values(context) {
    calls.push(context);
    return values;
  },
});

test("providers merge in registration order and hand-entered values win", async (t) => {
  const { projects, project } = await fixture(t);
  const calls = [];
  projects.addEnvironmentProvider(
    provider({ A: "first", B: "first", HAND: "first", BLANK: "first" }, calls),
  );
  projects.addEnvironmentProvider(provider({ B: "second", C: "second" }));
  assert.deepEqual(
    await projects.environmentFor(project.id, "agent", "local"),
    {
      A: "first",
      B: "second",
      C: "second",
      HAND: "hand-value",
      BLANK: "first",
    },
  );
  assert.equal(calls.length, 1);
  assert.equal(calls[0].project.id, project.id);
  assert.equal(calls[0].stage, "agent");
  assert.equal(calls[0].host, "local");
});

test("a throwing or rejecting provider is skipped", async (t) => {
  const { projects, project } = await fixture(t);
  projects.addEnvironmentProvider({
    values() {
      throw new Error("sync failure");
    },
  });
  projects.addEnvironmentProvider({
    values: () => Promise.reject(new Error("async failure")),
  });
  projects.addEnvironmentProvider(provider({ OK: "yes" }));
  assert.deepEqual(
    await projects.environmentFor(project.id, "agent", "local"),
    {
      OK: "yes",
      HAND: "hand-value",
    },
  );
});

test(
  "a provider that hangs is skipped after the timeout",
  { timeout: 30000 },
  async (t) => {
    const { projects, project } = await fixture(t, { providerTimeoutMs: 40 });
    let asked = false;
    projects.addEnvironmentProvider({
      values: () => {
        asked = true;
        return new Promise(() => {});
      },
    });
    projects.addEnvironmentProvider(provider({ OK: "yes" }));
    // The provider timeout runs on a mocked clock. Only the 40 ms option, not
    // the 30 s default, lets this one tick reach it.
    t.mock.timers.enable({ apis: ["setTimeout"] });
    const result = projects.environmentFor(project.id, "agent", "local");
    // The timer is set just before the provider is asked.
    await untilTurns(() => asked, "the hanging provider to be asked");
    t.mock.timers.tick(40);
    assert.deepEqual(await result, { OK: "yes", HAND: "hand-value" });
  },
);

test("no provider is asked for a withheld host", async (t) => {
  const { projects, project } = await fixture(t);
  const calls = [];
  projects.addEnvironmentProvider(provider({ A: "x" }, calls));
  await projects.setHostWithheld(project.id, "ssh:box", true);
  assert.deepEqual(
    await projects.environmentFor(project.id, "agent", "ssh:box"),
    {},
  );
  assert.equal(calls.length, 0);
  assert.equal(
    (await projects.environmentFor(project.id, "agent", "ssh:other")).A,
    "x",
  );
  assert.equal(calls.length, 1);
});

test("environmentChanged refreshes the daemons' copies", async (t) => {
  // The 250 ms debounce runs on a mocked clock: no wall-clock margin to lose under load.
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { projects } = await fixture(t);
  t.mock.timers.tick(250);
  let changes = 0;
  projects.onChange = () => (changes += 1);
  projects.environmentChanged();
  projects.environmentChanged();
  t.mock.timers.tick(249);
  assert.equal(changes, 0, "refreshed before the debounce elapsed");
  t.mock.timers.tick(1);
  assert.equal(changes, 1);
});

test("retireField hands the values over, then drops the field", async (t) => {
  const { projects, project } = await fixture(t);
  const { appDb } = require("../electron/app-db.cjs");
  const row = appDb(projects.userDataDir)
    .prepare("SELECT data FROM projects WHERE id = ?")
    .get(project.id);
  const write = (extra) =>
    appDb(projects.userDataDir)
      .prepare("UPDATE projects SET data = ? WHERE id = ?")
      .run(JSON.stringify({ ...JSON.parse(row.data), ...extra }), project.id);
  write({ legacy: { a: 1 } });
  await assert.rejects(
    projects.retireField("legacy", async () => {
      throw new Error("not stored");
    }),
    /not stored/,
  );
  const kept = appDb(projects.userDataDir)
    .prepare("SELECT data FROM projects WHERE id = ?")
    .get(project.id);
  assert.deepEqual(JSON.parse(kept.data).legacy, { a: 1 });
  let received;
  await projects.retireField("legacy", async (map) => {
    received = map;
  });
  assert.deepEqual(received, { [project.id]: { a: 1 } });
  const gone = appDb(projects.userDataDir)
    .prepare("SELECT data FROM projects WHERE id = ?")
    .get(project.id);
  assert.equal("legacy" in JSON.parse(gone.data), false);
});
