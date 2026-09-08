const test = require("node:test"),
  assert = require("node:assert/strict"),
  fs = require("node:fs/promises"),
  path = require("node:path"),
  os = require("node:os");
const { HermesScheduler } = require("../electron/agents/hermes-scheduler.cjs");
test("automatic scheduler is opt-in, uses native Desktop mode and persists the preference", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "sushiai-scheduler-"));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  let starts = 0,
    closes = 0;
  const options = [];
  const factory = (o) => {
    options.push(o);
    return {
      start: async () => {
        starts++;
        o.onState({ state: "ready" });
      },
      close: async () => {
        closes++;
      },
    };
  };
  const file = path.join(root, "scheduler.json"),
    scheduler = new HermesScheduler({ file, factory, publish: () => {} });
  assert.equal(starts, 0);
  await scheduler.set(true);
  await scheduler.set(true);
  assert.equal(starts, 1);
  assert.equal(options[0].env.HERMES_DESKTOP, "1");
  assert.equal(options[0].profile, "default");
  await scheduler.close();
  assert.ok(closes > 0);
  const restored = new HermesScheduler({ file, factory, publish: () => {} });
  await new Promise((r) => setImmediate(r));
  await restored.queue;
  assert.equal(starts, 2);
  await restored.set(false);
  await restored.close();
  assert.equal(JSON.parse(await fs.readFile(file, "utf8")).enabled, false);
});
test("closing before startup prevents an orphan scheduler", async () => {
  let starts = 0;
  const scheduler = new HermesScheduler({
    factory: () => ({
      start: async () => {
        starts++;
      },
      close: async () => {},
    }),
    publish: () => {},
  });
  const start = scheduler.set(true);
  await scheduler.close();
  await assert.rejects(start, /closed/);
  assert.equal(starts, 0);
});
