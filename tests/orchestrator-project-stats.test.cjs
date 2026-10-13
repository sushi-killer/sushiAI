const { test } = require("node:test");
const assert = require("node:assert/strict");

const scope = import("../src/orchestrator/projectScope.ts");

const project = {
  id: "p",
  folders: [
    { endpoint: "local", cwd: "/Users/a/app" },
    { endpoint: "ssh:lab", cwd: "/home/a/sushiai/app" },
    { endpoint: "ssh:gone", cwd: "/srv/app" },
    { endpoint: "", cwd: "/Users/a/app" },
    { endpoint: "local", cwd: "/Users/a/app-2" },
  ],
};
const lab = { host: "ssh:lab", repo: "/home/a/sushiai/app" };
const mac = { host: "local", repo: "/Users/a/app" };

function row(key, costUsd, runs, input, cached, output) {
  const tokens = { input, cached, output };
  return { key, costUsd, runs, tokens, cacheHitRate: 0 };
}

function total(rows, pick) {
  return rows.reduce((sum, r) => sum + pick(r), 0);
}

function summary(rows) {
  const totals = {
    costUsd: total(rows, (r) => r.costUsd),
    runs: total(rows, (r) => r.runs),
    tokens: {
      input: total(rows, (r) => r.tokens.input),
      cached: total(rows, (r) => r.tokens.cached),
      output: total(rows, (r) => r.tokens.output),
    },
    cacheHitRate: 0,
  };
  return { rows, totals };
}

function fakeClients(hosts) {
  const calls = [];
  const clientFor = (host) => ({
    taskList: async (repo, includeArchived) => {
      calls.push({ host, repo, includeArchived });
      if (hosts[host].fail) throw new Error("down");
      return hosts[host].tasks;
    },
    costsSummary: async (params) => {
      calls.push({ host, ...params });
      if (hosts[host].fail) throw new Error("down");
      return hosts[host].spend;
    },
  });
  return { calls, clientFor };
}

test("projectSources lists the current folder first, then every other folder once", async () => {
  const { projectSources } = await scope;
  const sources = projectSources(project, lab, ["local", "ssh:lab"]);
  assert.deepEqual(sources, [
    lab,
    mac,
    { host: "local", repo: "/Users/a/app-2" },
  ]);
});

test("projectSources without a project is just the current folder", async () => {
  const { projectSources } = await scope;
  assert.deepEqual(projectSources(null, mac), [mac]);
});

test("projectSources keeps the current folder when the host list omits it", async () => {
  const { projectSources } = await scope;
  const here = { host: "local", repo: "/elsewhere" };
  assert.deepEqual(projectSources(project, here, ["ssh:lab"]), [here, lab]);
});

test("mergeSpend sums days across hosts and recomputes the cache hit rate", async () => {
  const { mergeSpend } = await scope;
  const a = summary([
    row("2026-09-28", 1, 2, 100, 300, 10),
    row("2026-09-29", 2, 1, 0, 0, 5),
  ]);
  const b = summary([
    row("2026-09-29", 0.5, 3, 100, 100, 7),
    row("2026-09-30", 4, 1, 0, 0, 0),
  ]);
  const merged = mergeSpend([b, a], "day");
  assert.deepEqual(
    merged.rows.map((r) => r.key),
    ["2026-09-28", "2026-09-29", "2026-09-30"],
  );
  const middle = merged.rows[1];
  assert.equal(middle.costUsd, 2.5);
  assert.equal(middle.runs, 4);
  assert.deepEqual(middle.tokens, { input: 100, cached: 100, output: 12 });
  assert.equal(middle.cacheHitRate, 0.5);
  assert.equal(merged.totals.costUsd, 7.5);
  assert.equal(merged.totals.runs, 7);
  assert.deepEqual(merged.totals.tokens, {
    input: 200,
    cached: 400,
    output: 22,
  });
  assert.ok(Math.abs(merged.totals.cacheHitRate - 2 / 3) < 1e-9);
});

test("mergeSpend orders other groupings by cost, highest first", async () => {
  const { mergeSpend } = await scope;
  const a = summary([
    row("review", 1, 1, 0, 0, 0),
    row("implement", 3, 1, 0, 0, 0),
  ]);
  const b = summary([row("review", 4, 1, 0, 0, 0)]);
  const merged = mergeSpend([a, b], "stage");
  assert.deepEqual(
    merged.rows.map((r) => [r.key, r.costUsd]),
    [
      ["review", 5],
      ["implement", 3],
    ],
  );
});

test("mergeSpend of nothing is an empty summary", async () => {
  const { mergeSpend } = await scope;
  const empty = mergeSpend([], "day");
  assert.deepEqual(empty.rows, []);
  assert.equal(empty.totals.costUsd, 0);
  assert.equal(empty.totals.runs, 0);
  assert.equal(empty.totals.cacheHitRate, 0);
});

test("loadProjectTasks asks each host for its own folder and joins the tasks", async () => {
  const { loadProjectTasks } = await scope;
  const { calls, clientFor } = fakeClients({
    local: { tasks: [{ id: "t1", repo: mac.repo }] },
    "ssh:lab": { tasks: [{ id: "t2", repo: lab.repo, host: "ssh:lab" }] },
  });
  const result = await loadProjectTasks(clientFor, [mac, lab], true);
  assert.deepEqual(
    result.tasks.map((t) => t.id),
    ["t1", "t2"],
  );
  assert.deepEqual(result.failed, []);
  assert.deepEqual(calls, [
    { host: "local", repo: mac.repo, includeArchived: true },
    { host: "ssh:lab", repo: lab.repo, includeArchived: true },
  ]);
});

test("loadProjectTasks keeps going when one host is unreachable", async () => {
  const { loadProjectTasks } = await scope;
  const { clientFor } = fakeClients({
    local: { tasks: [{ id: "t1", repo: mac.repo }] },
    "ssh:lab": { fail: true },
  });
  const result = await loadProjectTasks(clientFor, [mac, lab], true);
  assert.deepEqual(
    result.tasks.map((t) => t.id),
    ["t1"],
  );
  assert.deepEqual(result.failed, ["ssh:lab"]);
});

test("loadProjectSpend rejects only when every host fails", async () => {
  const { loadProjectSpend } = await scope;
  const { clientFor } = fakeClients({
    local: { fail: true },
    "ssh:lab": { fail: true },
  });
  const query = { groupBy: ["day"] };
  await assert.rejects(loadProjectSpend(clientFor, [mac, lab], query), /down/);
});

test("loadProjectSpend queries each host by its folder and merges the answers", async () => {
  const { loadProjectSpend } = await scope;
  const { calls, clientFor } = fakeClients({
    local: { spend: summary([row("2026-09-29", 1, 1, 0, 0, 0)]) },
    "ssh:lab": { spend: summary([row("2026-09-29", 2, 2, 0, 0, 0)]) },
  });
  const query = { from: "2026-09-28", to: "2026-10-04", groupBy: ["day"] };
  const result = await loadProjectSpend(clientFor, [mac, lab], query);
  assert.equal(result.summary.totals.costUsd, 3);
  assert.equal(result.summary.rows.length, 1);
  assert.equal(result.summary.rows[0].runs, 3);
  assert.deepEqual(result.failed, []);
  assert.deepEqual(
    calls.map((c) => [c.host, c.repo, c.from, c.to]),
    [
      ["local", mac.repo, "2026-09-28", "2026-10-04"],
      ["ssh:lab", lab.repo, "2026-09-28", "2026-10-04"],
    ],
  );
});
