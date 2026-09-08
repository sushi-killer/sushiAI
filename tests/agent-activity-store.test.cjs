const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { ActivityStore } = require("../electron/agents/activity-store.cjs");

test("activity survives restart, ordered writes retain the newest state and private permissions", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "sushiai-activity-"));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const file = path.join(root, "agents", "activity.json");
  const store = new ActivityStore(file);
  const entry = {
    id: "native-effect",
    title: "Memory added",
    createdAt: 1,
    read: false,
  };
  store.save([entry]);
  store.save([{ ...entry, read: true }]);
  await store.close();
  const restored = new ActivityStore(file);
  assert.deepEqual(restored.entries, [{ ...entry, read: true }]);
  assert.equal((await fs.stat(file)).mode & 0o777, 0o600);
  assert.deepEqual(await fs.readdir(path.dirname(file)), ["activity.json"]);
});

test("corrupt saved activity does not prevent live activity or startup", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "sushiai-activity-"));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const file = path.join(root, "activity.json");
  await fs.writeFile(file, "invalid");
  const store = new ActivityStore(file);
  assert.match(store.error, /could not be loaded/);
  store.save([{ id: "new", title: "Skill improved", createdAt: 2 }]);
  await store.close();
  assert.equal(store.error, null);
  assert.equal(new ActivityStore(file).entries[0].id, "new");
});
