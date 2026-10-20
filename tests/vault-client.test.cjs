const { test } = require("node:test");
const assert = require("node:assert/strict");

test("onProjectChange fires after a saved projectSet and stops after unsubscribe", async () => {
  let fail = false;
  globalThis.window = {
    bridge: {
      moduleInvoke: async () => {
        if (fail) throw new Error("rejected");
        return { mount: "secret", paths: ["team/app"] };
      },
    },
  };
  const { onProjectChange, projectSet } =
    await import("../src/vault/client.ts");
  let calls = 0;
  const off = onProjectChange(() => calls++);
  fail = true;
  await assert.rejects(projectSet("p1", { paths: ["team/app"] }));
  assert.equal(calls, 0);
  fail = false;
  await projectSet("p1", { paths: ["team/app"] });
  assert.equal(calls, 1);
  off();
  await projectSet("p1", null);
  assert.equal(calls, 1);
  delete globalThis.window;
});
