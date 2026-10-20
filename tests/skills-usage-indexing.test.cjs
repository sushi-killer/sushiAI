const assert = require("node:assert/strict");
const Module = require("node:module");
const test = require("node:test");
const { turn } = require("./helpers/deadline.cjs");

const originalLoad = Module._load;
Module._load = function (request, ...rest) {
  if (request === "electron") return { powerSaveBlocker: {} };
  return originalLoad.call(this, request, ...rest);
};
const { registerAppIpc } = require("../electron/ipc/app.cjs");
Module._load = originalLoad;

/** Registers the app IPC with a stubbed scanner; `scans` records each call. */
function setup() {
  const handlers = {};
  const sent = [];
  const scans = [];
  const win = {
    isDestroyed: () => false,
    webContents: { send: (channel) => sent.push(channel) },
  };
  registerAppIpc({
    handle: (name, fn) => (handlers[name] = fn),
    app: {},
    getMainWindow: () => win,
    scanLocalSkills: async (options) => {
      scans.push(options);
      return [{ name: "vault" }];
    },
    userDataDir: () => "/tmp/sushiai-test-data",
  });
  return { catalog: handlers.catalog, sent, scans };
}

async function withEnv(value, run) {
  const before = process.env.SUSHIAI_TEST_WINDOW;
  if (value === undefined) delete process.env.SUSHIAI_TEST_WINDOW;
  else process.env.SUSHIAI_TEST_WINDOW = value;
  try {
    await run();
  } finally {
    if (before === undefined) delete process.env.SUSHIAI_TEST_WINDOW;
    else process.env.SUSHIAI_TEST_WINDOW = before;
  }
}

test("usage is indexed on the first catalog and on every forced refresh", () =>
  withEnv(undefined, async () => {
    const { catalog, scans } = setup();
    await catalog("skills");
    assert.equal(scans[0].indexUsage, true);
    await catalog("skills");
    assert.equal(scans.length, 1, "a cached catalog scans nothing");
    await catalog("skills", { force: true });
    assert.equal(scans[1].indexUsage, true);
    await catalog("skills", { force: true });
    assert.equal(scans[2].indexUsage, true);
  }));

test("a launch under SUSHIAI_TEST_WINDOW never indexes", () =>
  withEnv("hidden", async () => {
    const { catalog, scans } = setup();
    await catalog("skills");
    await catalog("skills", { force: true });
    assert.deepEqual(
      scans.map((s) => s.indexUsage),
      [false, false],
    );
  }));

test("the rescan after indexing does not index again and the window hears of it", () =>
  withEnv(undefined, async () => {
    const { catalog, scans, sent } = setup();
    await catalog("skills");
    scans[0].onIndexed();
    await turn();
    await turn();
    assert.equal(scans.length, 2);
    assert.equal(scans[1].indexUsage, false);
    assert.deepEqual(sent, ["skills-usage-indexed"]);
  }));

test("a failed first catalog does not use up the first-run index", () =>
  withEnv(undefined, async () => {
    let calls = 0;
    const handlers = {};
    registerAppIpc({
      handle: (name, fn) => (handlers[name] = fn),
      app: {},
      getMainWindow: () => null,
      scanLocalSkills: async (options) => {
        calls += 1;
        if (calls === 1) throw new Error("no binary");
        assert.equal(options.indexUsage, true);
        return [];
      },
      userDataDir: () => "/tmp/sushiai-test-data",
    });
    await assert.rejects(handlers.catalog("skills"), /no binary/);
    await handlers.catalog("skills");
    assert.equal(calls, 2);
  }));
