const { test } = require("node:test");
const assert = require("node:assert/strict");

const store = import("../src/orchestrator/useTasks.ts");

const task = { id: "t1", title: "T", status: "running", repo: "/r" };
const tick = () => new Promise((resolve) => setTimeout(resolve, 5));

test("the task list refetches as soon as the module becomes available", async () => {
  const { startTaskStore, currentTasks } = await store;
  let available = false;
  let daemon = () => {};
  let lists = 0;
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
  await tick();
  assert.deepEqual(currentTasks(), []);
  assert.equal(lists, 0);

  // A daemon state without the capability changes nothing.
  daemon({ host: "local", state: "ready", capabilities: [], generation: 1 });
  await tick();
  assert.equal(lists, 0);

  available = true;
  daemon({
    host: "local",
    state: "ready",
    capabilities: ["orch"],
    generation: 2,
  });
  await tick();
  assert.equal(lists, 1);
  assert.equal(currentTasks().length, 1);
  stop();
  delete globalThis.window;
});
