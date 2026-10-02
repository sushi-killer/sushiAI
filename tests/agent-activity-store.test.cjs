const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { writeStore } = require("../electron/app-db.cjs");
const { ActivityStore } = require("../electron/agents/activity-store.cjs");

test("activity survives restart and ordered writes retain the newest state", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "sushiai-activity-"));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const store = new ActivityStore(root);
  const entry = {
    id: "native-effect",
    title: "Memory added",
    createdAt: 1,
    read: false,
  };
  const other = { id: "second", title: "Skill improved", createdAt: 2 };
  store.save([entry, other]);
  store.save([{ ...entry, read: true }, other]);
  await store.close();
  const restored = new ActivityStore(root);
  assert.deepEqual(restored.entries, [{ ...entry, read: true }, other]);
  assert.equal(
    (await fs.stat(path.join(root, "sushiai.db"))).mode & 0o777,
    0o600,
  );
});

test("a legacy activity journal is imported once and an invalid one never blocks startup", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "sushiai-activity-"));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  await fs.mkdir(path.join(root, "agents"));
  const file = path.join(root, "agents", "hermes-activity.json");
  await fs.writeFile(
    file,
    JSON.stringify([{ id: "old", title: "Imported", createdAt: 3 }]),
  );
  assert.equal(new ActivityStore(root).entries[0].id, "old");
  assert.deepEqual(await fs.readdir(path.dirname(file)), [
    "hermes-activity.json.imported",
  ]);
  const bad = await fs.mkdtemp(path.join(os.tmpdir(), "sushiai-activity-"));
  t.after(() => fs.rm(bad, { recursive: true, force: true }));
  await fs.mkdir(path.join(bad, "agents"));
  const warn = console.warn;
  console.warn = () => {};
  t.after(() => {
    console.warn = warn;
  });
  await fs.writeFile(
    path.join(bad, "agents", "hermes-activity.json"),
    "invalid",
  );
  const store = new ActivityStore(bad);
  assert.deepEqual(store.entries, []);
  store.save([{ id: "new", title: "Skill improved", createdAt: 2 }]);
  await store.close();
  assert.equal(store.error, null);
  assert.equal(new ActivityStore(bad).entries[0].id, "new");
  assert.equal(
    (await fs.readdir(path.join(bad, "agents"))).includes(
      "hermes-activity.json",
    ),
    true,
  );
});

test("a stored journal with an invalid entry reports it and still takes live activity", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "sushiai-activity-"));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  writeStore(root, "hermes-activity", { order: ["x"], "e:x": { id: "x" } });
  const store = new ActivityStore(root);
  assert.match(store.error, /could not be loaded/);
  store.save([{ id: "new", title: "Skill improved", createdAt: 2 }]);
  await store.close();
  assert.equal(store.error, null);
  assert.equal(new ActivityStore(root).entries[0].id, "new");
});
