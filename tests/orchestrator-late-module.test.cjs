const { test } = require("node:test");
const assert = require("node:assert/strict");
const { until } = require("./helpers/deadline.cjs");

const store = import("../src/orchestrator/useTasks.ts");

const task = { id: "t1", title: "T", status: "running", repo: "/r" };

test("the task list refetches as soon as the module becomes available", async (t) => {
  const { startTaskStore, currentTasks } = await store;
  let available = false;
  let daemon = () => {};
  let lists = 0;
  let probes = 0;
  globalThis.window = {
    bridge: {
      orchestratorHosts: async () => [],
      onOrchestratorHosts: () => () => {},
      onOrchestrator: () => () => {},
      onDaemonState: (callback) => {
        daemon = callback;
        return () => {};
      },
      orchestratorProbe: async () => {
        probes++;
        if (!available) throw new Error("orch capability missing");
        return { pid: 1 };
      },
      orchestrator: async (method) => {
        assert.equal(method, "task.list");
        lists++;
        return [task];
      },
    },
  };
  const stop = startTaskStore();
  t.after(() => {
    stop();
    delete globalThis.window;
  });
  await until(() => probes >= 1, "the first probe");
  assert.deepEqual(currentTasks(), []);
  assert.equal(lists, 0);

  // A daemon state without the capability changes nothing.
  daemon({ host: "local", state: "ready", capabilities: [], generation: 1 });
  assert.equal(lists, 0);

  available = true;
  daemon({
    host: "local",
    state: "ready",
    capabilities: ["orch"],
    generation: 2,
  });
  await until(() => currentTasks().length === 1, "the task list");
  // One probe at start, one for the capable state; the capability-less state
  // before it started none.
  assert.equal(probes, 2);
  assert.equal(lists, 1);
});
