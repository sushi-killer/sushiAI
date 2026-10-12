const { test } = require("node:test");
const assert = require("node:assert/strict");
const { createModuleIpc } = require("../electron/extensions/module-ipc.cjs");

function setup() {
  const handlers = new Map();
  const moduleIpc = createModuleIpc({
    handle: (channel, callback) => {
      assert.equal(handlers.has(channel), false);
      handlers.set(channel, callback);
    },
  });
  assert.deepEqual([...handlers.keys()], ["module:invoke"]);
  return { moduleIpc, invoke: handlers.get("module:invoke") };
}

test("dispatch passes args through and returns sync and async method values", async () => {
  const { moduleIpc, invoke } = setup();
  const payload = { value: 42 };
  const calls = [];
  moduleIpc.register("builtin.demo", {
    sync(...args) {
      calls.push(args);
      return payload;
    },
    async async(...args) {
      calls.push(args);
      return "done";
    },
  });
  moduleIpc.register("builtin.other", { sync: () => "other" });

  assert.equal(await invoke("builtin.demo", "sync", payload, 3), payload);
  assert.equal(await invoke("builtin.demo", "async", "input", null), "done");
  assert.equal(await invoke("builtin.other", "sync"), "other");
  assert.deepEqual(calls, [
    [payload, 3],
    ["input", null],
  ]);
  assert.equal(calls[0][0], payload);
});

test("unknown and non-string module ids reject with Unknown module", async () => {
  const { moduleIpc, invoke } = setup();
  moduleIpc.register("builtin.demo", { run: () => "done" });
  await assert.rejects(invoke("missing", "run"), {
    message: "Unknown module: missing",
  });
  for (const id of [
    undefined,
    null,
    42,
    {},
    ["builtin.demo"],
    Object.create(null),
    { toString: "not callable" },
  ])
    await assert.rejects(invoke(id, "run"), /Unknown module/);
});

test("unknown, inherited, non-function and non-string methods reject with Unknown method", async () => {
  const { moduleIpc, invoke } = setup();
  moduleIpc.register(
    "builtin.demo",
    Object.assign(Object.create({ inherited: () => "inherited" }), {
      run: () => "done",
      value: 42,
    }),
  );
  for (const method of [
    "missing",
    "constructor",
    "__proto__",
    "toString",
    "inherited",
    "value",
  ])
    await assert.rejects(invoke("builtin.demo", method), {
      message: `Unknown method: builtin.demo.${method}`,
    });
  for (const method of [
    undefined,
    null,
    42,
    {},
    ["run"],
    Object.create(null),
    { toString: "not callable" },
  ])
    await assert.rejects(invoke("builtin.demo", method), /Unknown method/);
});

test("registering the same module twice throws and keeps the first table", async () => {
  const { moduleIpc, invoke } = setup();
  moduleIpc.register("builtin.demo", { run: () => "first" });
  assert.throws(
    () => moduleIpc.register("builtin.demo", { run: () => "second" }),
    /Duplicate module/,
  );
  assert.equal(await invoke("builtin.demo", "run"), "first");
});

test("a method's own sync or async error passes through unchanged", async () => {
  const { moduleIpc, invoke } = setup();
  const error = new Error("method failed");
  moduleIpc.register("builtin.demo", {
    sync() {
      throw error;
    },
    async async() {
      throw error;
    },
  });
  for (const method of ["sync", "async"])
    await assert.rejects(
      invoke("builtin.demo", method),
      (reason) => reason === error,
    );
});
