const { test } = require("node:test");
const assert = require("node:assert/strict");

const load = () => import("../src/orchestrator/ownerAttention.ts");
const task = (over = {}) => ({
  id: "t1",
  title: "T",
  repo: "/w/sushiai/",
  status: "done",
  archived: false,
  decisions: [],
  attempts: [],
  updatedAt: 1000,
  baseRef: "main",
  ...over,
});

test("landTasks lists finished, unlanded top-level tasks and leaves the badge alone", async () => {
  const { landTasks, needsOwner } = await load();
  const tasks = [
    task({ id: "a", updatedAt: 1 }),
    task({ id: "b", updatedAt: 2 }),
    task({ id: "landed", landedSha: "abc" }),
    task({ id: "sub", parent: "a" }),
    task({ id: "old", archived: true }),
    task({ id: "nobase", baseRef: undefined }),
    task({ id: "run", status: "running" }),
  ];
  assert.deepEqual(
    landTasks(tasks).map((t) => t.id),
    ["b", "a"],
  );
  assert.equal(tasks.filter(needsOwner).length, 0);
});

test("ownerKind splits questions from decisions", async () => {
  const { ownerKind } = await load();
  assert.equal(ownerKind(task({ status: "waiting" })), "answer");
  assert.equal(ownerKind(task({ status: "failed" })), "decide");
  assert.equal(ownerKind(task({ status: "landing" })), "decide");
});

test("inboxZeroSummary counts today's landings and running tasks", async () => {
  const { inboxZeroSummary } = await load();
  const now = new Date(2026, 8, 29, 15).getTime();
  const tasks = [
    task({ id: "a", landedSha: "x", updatedAt: now - 1000 }),
    task({ id: "b", landedSha: "y", updatedAt: now - 2000 }),
    task({ id: "old", landedSha: "z", updatedAt: now - 3 * 86_400_000 }),
    task({ id: "r", status: "running" }),
    task({ id: "r2", status: "running", archived: true }),
  ];
  assert.equal(
    inboxZeroSummary(tasks, now),
    "Nothing needs you. 2 tasks landed today; 1 is still running.",
  );
  assert.equal(inboxZeroSummary([], now), "Nothing needs you.");
});

test("projectName is the last path segment", async () => {
  const { projectName } = await load();
  assert.equal(projectName("/w/sushiai/"), "sushiai");
  assert.equal(projectName("C:\\w\\sushiai"), "sushiai");
});

test("needsOwner counts waiting, failed, landing and engine stops, never owner stops or archived", async () => {
  const { needsOwner, ownerTasks } = await load();
  const failedAttempt = { status: "failed", failure: { kind: "verify" } };
  const tasks = [
    task({ id: "wait", status: "waiting", updatedAt: 3 }),
    task({ id: "fail", status: "failed", updatedAt: 2 }),
    task({ id: "land", status: "landing", updatedAt: 1 }),
    task({
      id: "engine",
      status: "stopped",
      decisions: ["Orchestrator: budget spent"],
      updatedAt: 4,
    }),
    task({
      id: "crashed",
      status: "stopped",
      attempts: [failedAttempt],
      updatedAt: 0,
    }),
    task({ id: "owner", status: "stopped", decisions: ["Owner: stop"] }),
    task({
      id: "interrupted",
      status: "stopped",
      attempts: [{ status: "interrupted", failure: { kind: "x" } }],
    }),
    task({ id: "gone", status: "waiting", archived: true }),
    task({ id: "run", status: "running" }),
  ];
  assert.deepEqual(
    tasks.filter(needsOwner).map((t) => t.id),
    ["wait", "fail", "land", "engine", "crashed"],
  );
  assert.deepEqual(
    ownerTasks(tasks).map((t) => t.id),
    ["engine", "wait", "fail", "land", "crashed"],
  );
});
