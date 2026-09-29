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

test("inboxHeadline reads like the concept, with singular forms", async () => {
  const { inboxHeadline } = await load();
  assert.equal(
    inboxHeadline(10, 4, 2, 3_600_000),
    "10 things need you across 4 projects on 2 hosts · oldest 1h",
  );
  assert.equal(
    inboxHeadline(1, 1, 1, 120_000),
    "1 thing needs you across 1 project on 1 host · oldest 2m",
  );
  assert.equal(inboxHeadline(0, 0, 0, null), "Nothing needs you");
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

test("composeAnswer merges a pick and a note", async () => {
  const { composeAnswer } = await load();
  assert.equal(composeAnswer("dark", ""), "dark");
  assert.equal(composeAnswer("", " hi "), "hi");
  assert.equal(composeAnswer("dark", "but soft"), "dark: but soft");
  assert.equal(composeAnswer("", ""), "");
});

test("stepSelection clamps and falls back to the first key", async () => {
  const { stepSelection } = await load();
  const keys = ["a", "b", "c"];
  assert.equal(stepSelection(keys, "a", 1), "b");
  assert.equal(stepSelection(keys, "c", 1), "c");
  assert.equal(stepSelection(keys, "a", -1), "a");
  assert.equal(stepSelection(keys, "gone", 1), "a");
  assert.equal(stepSelection(keys, null, 1), "a");
  assert.equal(stepSelection([], "a", 1), null);
});

test("projectName is the last path segment", async () => {
  const { projectName } = await load();
  assert.equal(projectName("/w/sushiai/"), "sushiai");
  assert.equal(projectName("C:\\w\\sushiai"), "sushiai");
});

test("elapsedLabel picks minutes, hours or days", async () => {
  const { elapsedLabel } = await load();
  assert.equal(elapsedLabel(2 * 60_000), "2m");
  assert.equal(elapsedLabel(90 * 60_000), "1h");
  assert.equal(elapsedLabel(50 * 3600_000), "2d");
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
