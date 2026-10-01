const { test } = require("node:test");
const assert = require("node:assert/strict");
const { SnapshotCoordinator } = require("../src/herdrSync.ts");
const { HerdrSnapshots } = require("../electron/herdr-snapshots.cjs");
const { ipcResult } = require("../electron/ipc/errors.cjs");

const tick = () => new Promise((resolve) => setImmediate(resolve));
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function renderer() {
  const requests = [],
    applied = [],
    errors = [];
  const coordinator = new SnapshotCoordinator(
    () => {
      const request = deferred();
      requests.push(request);
      return request.promise;
    },
    (value) => applied.push(value),
    (error) => errors.push(error),
  );
  return { coordinator, requests, applied, errors };
}

test("twenty refreshes share one flight and one trailing snapshot", async () => {
  const { coordinator, requests, applied } = renderer();
  const running = coordinator.refresh();
  await tick();
  for (let i = 0; i < 20; i++) assert.equal(coordinator.refresh(), running);
  assert.equal(requests.length, 1);
  requests[0].resolve("first");
  await tick();
  assert.equal(requests.length, 2);
  requests[1].resolve("latest");
  await running;
  assert.deepEqual(applied, ["first", "latest"]);
});

test("an invalidated old response cannot end the newly committed pane", async () => {
  const { coordinator, requests, applied } = renderer();
  const running = coordinator.refresh();
  await tick();
  coordinator.invalidate();
  requests[0].resolve({ panes: [] });
  await tick();
  assert.deepEqual(applied, []);
  assert.equal(requests.length, 2);
  requests[1].resolve({ panes: ["new-pane"] });
  await running;
  assert.deepEqual(applied, [{ panes: ["new-pane"] }]);
});

test("disconnect and reconnect reject previous connection responses", async () => {
  const { coordinator, requests, applied, errors } = renderer();
  const running = coordinator.refresh();
  await tick();
  coordinator.disconnect();
  const reconnect = coordinator.reconnect();
  assert.equal(reconnect, running);
  requests[0].reject(new Error("previous socket"));
  await tick();
  assert.equal(errors.length, 0);
  requests[1].resolve("reconnected full snapshot");
  await running;
  assert.deepEqual(applied, ["reconnected full snapshot"]);
});

test("closing a coordinator prevents obsolete state changes", async () => {
  const { coordinator, requests, applied } = renderer();
  const running = coordinator.refresh();
  await tick();
  coordinator.close();
  requests[0].resolve("old endpoint");
  await running;
  assert.deepEqual(applied, []);
  await coordinator.refresh();
  assert.equal(requests.length, 1);
});

test("independent endpoints do not block each other's snapshots", async () => {
  const a = renderer(),
    b = renderer();
  const first = a.coordinator.refresh(),
    second = b.coordinator.refresh();
  await tick();
  b.requests[0].resolve("other host");
  await second;
  assert.deepEqual(b.applied, ["other host"]);
  assert.deepEqual(a.applied, []);
  a.requests[0].resolve("local");
  await first;
});

test("main process shares snapshots between launch lookup and renderer", async () => {
  const requests = [];
  const pool = new HerdrSnapshots({
    getConnections: () => ({ socket: async (path) => path }),
    rpc: () => {
      const current = deferred();
      requests.push(current);
      return current.promise;
    },
  });
  const read = pool.read("/tmp/isolated.sock");
  await tick();
  assert.equal(pool.read("/tmp/isolated.sock"), read);
  pool.invalidate("/tmp/isolated.sock");
  requests[0].resolve("stale");
  await tick();
  assert.equal(requests.length, 2);
  requests[1].resolve("latest");
  assert.equal(await read, "latest");
});

test("IPC errors retain machine-readable codes and successful values", async () => {
  const error = Object.assign(new Error("localised message can change"), {
    code: "pane_not_found",
    data: { paneId: "missing" },
    retryable: true,
  });
  const result = await ipcResult(() => {
    throw error;
  }, []);
  assert.deepEqual(result, {
    __sushiaiIpc: 1,
    error: {
      code: "pane_not_found",
      message: error.message,
      data: { paneId: "missing" },
      retryable: true,
    },
  });
  assert.equal("stack" in result.error, false);
  assert.deepEqual(await ipcResult((input) => input, ["ok"]), {
    __sushiaiIpc: 1,
    value: "ok",
  });
});

test("snapshots keep the intended agent and retry state after failed preparation", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const endpoint = "/tmp/failed-preparation.sock";
  const current = [
    {
      id: "project",
      connection: endpoint,
      herdrId: "ws",
      name: "Project",
      cwd: "/work/project",
      layout: { type: "leaf", id: "created-pane" },
      panels: [
        {
          id: "created-pane",
          herdrId: "pane",
          kind: "agent",
          title: "Custom Claude",
          agent: "claude",
          launchOperationId: "retry",
          launchError: "Settings transfer failed",
        },
      ],
    },
  ];
  const next = reconcileHerdrWorkspaces(
    current,
    {
      workspaces: [{ workspace_id: "ws", label: "Project" }],
      panes: [
        {
          pane_id: "pane",
          workspace_id: "ws",
          agent_status: "idle",
          cwd: "/work/project",
        },
      ],
    },
    endpoint,
  );
  assert.equal(next[0].panels[0].kind, "agent");
  assert.equal(next[0].panels[0].agent, "claude");
  assert.equal(next[0].panels[0].title, "Custom Claude");
  assert.equal(next[0].panels[0].launchOperationId, "retry");
});
