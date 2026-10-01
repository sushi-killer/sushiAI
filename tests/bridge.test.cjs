const { test } = require("node:test");
const assert = require("node:assert/strict");
const { wrapBridge } = require("../src/bridge.ts");

test("renderer unwraps structured IPC after crossing contextBridge", async () => {
  const bridge = wrapBridge(
    Object.freeze({
      herdr: async () => ({
        __sushiaiIpc: 1,
        error: {
          message: "Missing pane",
          code: "pane_not_found",
          stage: "create",
          retryable: true,
          data: { paneId: "old" },
        },
      }),
      launch: async () => ({ __sushiaiIpc: 1, value: { paneId: "new" } }),
    }),
  );
  await assert.rejects(bridge.herdr(), (error) => {
    assert.ok(error instanceof Error);
    assert.equal(error.code, "pane_not_found");
    assert.equal(error.stage, "create");
    assert.equal(error.retryable, true);
    assert.deepEqual(error.data, { paneId: "old" });
    return true;
  });
  assert.deepEqual(await bridge.launch(), { paneId: "new" });
});

test("synchronous state and event unsubscription retain their return values", async () => {
  const unsubscribe = () => {};
  const bridge = wrapBridge({
    state: () => "saved state",
    subscribe: () => unsubscribe,
    legacy: async () => 12,
    flush: async () => ({ __sushiaiIpc: 1 }),
  });
  assert.equal(bridge.state(), "saved state");
  assert.equal(bridge.subscribe(), unsubscribe);
  assert.equal(await bridge.legacy(), 12);
  assert.equal(await bridge.flush(), undefined);
});
