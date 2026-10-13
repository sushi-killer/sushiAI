const test = require("node:test");
const assert = require("node:assert/strict");

const load = () => import("../src/orchestrator/projectSettings.ts");

const route = { id: "a", label: "A", harness: "claude" };

function settings(extra = {}) {
  return {
    routes: [route],
    tiers: { mechanical: "a", standard: "a", hard: "a" },
    review: "auto",
    autoAnswer: false,
    parallel: 2,
    experiments: { land: true, loopDetect: true, stallTimeoutSecs: 0 },
    worktreeRoot: "",
    ...extra,
  };
}

const orch = {
  defaults: {
    autoAnswer: true,
    tiers: { hard: "b" },
    experiments: { land: false },
  },
  hosts: {
    "ssh:lab": { autoAnswer: false, parallel: 7, tiers: { standard: "c" } },
  },
};

test("copySettings copies a host's settings but keeps the target's repo paths", async () => {
  const { copySettings } = await load();
  const other = { id: "x", label: "X", harness: "codex" };
  const source = settings({
    autoAnswer: true,
    parallel: 5,
    routes: [other],
    tiers: { mechanical: "x", standard: "x", hard: "x" },
    worktreeRoot: "/src/wt",
    sandboxRepos: { "/src/repo": "host" },
    landOnDefaultRepos: { "/src/repo": true },
  });
  const target = settings({
    parallel: 9,
    worktreeRoot: "/t/wt",
    sandboxRepos: { "/t/repo": "native" },
    futureKey: 1,
  });
  const before = JSON.stringify(target);
  const copy = copySettings(source, target);
  assert.equal(copy.autoAnswer, true);
  assert.equal(copy.parallel, 5);
  assert.deepEqual(copy.routes, [other]);
  assert.deepEqual(copy.tiers, source.tiers);
  assert.equal(copy.worktreeRoot, "/t/wt");
  assert.deepEqual(copy.sandboxRepos, { "/t/repo": "native" });
  assert.equal(copy.landOnDefaultRepos, undefined);
  assert.equal(copy.futureKey, 1);
  assert.equal(JSON.stringify(target), before);
});

test("effectiveSettings layers the project default, then the host override", async () => {
  const { effectiveSettings } = await load();
  const base = settings();
  const local = effectiveSettings(base, orch, "local");
  assert.equal(local.autoAnswer, true);
  assert.equal(local.parallel, 2);
  assert.deepEqual(local.tiers, { mechanical: "a", standard: "a", hard: "b" });
  assert.deepEqual(local.experiments, {
    land: false,
    loopDetect: true,
    stallTimeoutSecs: 0,
  });
  const lab = effectiveSettings(base, orch, "ssh:lab");
  assert.equal(lab.autoAnswer, false);
  assert.equal(lab.parallel, 7);
  assert.deepEqual(lab.tiers, { mechanical: "a", standard: "c", hard: "b" });
  assert.deepEqual(effectiveSettings(base, undefined, "local"), base);
  assert.equal(base.autoAnswer, false);
});

test("originOf says where a value comes from", async () => {
  const { originOf } = await load();
  assert.equal(originOf(orch, "ssh:lab", "autoAnswer"), "override");
  assert.equal(originOf(orch, "local", "autoAnswer"), "project");
  assert.equal(originOf(orch, "ssh:lab", "tiers.hard"), "project");
  assert.equal(originOf(orch, "ssh:lab", "tiers.standard"), "override");
  assert.equal(originOf(orch, "local", "tiers.standard"), "host");
  assert.equal(originOf(orch, "local", "experiments.land"), "project");
  assert.equal(originOf(orch, "local", "parallel"), "host");
  assert.equal(originOf(undefined, "local", "parallel"), "host");
});

test("an override is set and reset one value at a time, without mutating", async () => {
  const lib = await load();
  const project = lib.withProjectValue(undefined, "tiers.hard", "b");
  assert.deepEqual(project, { defaults: { tiers: { hard: "b" } } });
  const o = lib.withHostOverride(project, "ssh:lab", "autoAnswer", true);
  const before = JSON.stringify(o);
  assert.deepEqual(o.hosts, { "ssh:lab": { autoAnswer: true } });
  const reset = lib.withoutHostOverride(o, "ssh:lab", "autoAnswer");
  assert.equal(lib.originOf(reset, "ssh:lab", "autoAnswer"), "host");
  assert.equal(reset.hosts?.["ssh:lab"], undefined);
  assert.deepEqual(reset.defaults, { tiers: { hard: "b" } });
  assert.equal(JSON.stringify(o), before);
  let two = lib.withHostOverride(undefined, "ssh:lab", "tiers.hard", "x");
  two = lib.withHostOverride(two, "ssh:lab", "tiers.standard", "y");
  two = lib.withoutHostOverride(two, "ssh:lab", "tiers.hard");
  assert.deepEqual(two.hosts["ssh:lab"], { tiers: { standard: "y" } });
});

test("projectHostIds lists this Mac first, then every host the project knows", async () => {
  const { projectHostIds } = await load();
  assert.deepEqual(projectHostIds({}), ["local"]);
  const ids = projectHostIds({
    folders: [
      { endpoint: "ssh:b", cwd: "/x" },
      { endpoint: "local", cwd: "/y" },
    ],
    targets: ["ssh:a", "local"],
    hosts: { "ssh:c": { withheld: true } },
  });
  assert.deepEqual(ids, ["local", "ssh:a", "ssh:b", "ssh:c"]);
});
