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

test("composeAnswer: typed text replaces the pick, never prefixes it", async () => {
  const { composeAnswer } = await load();
  assert.equal(composeAnswer("dark", ""), "dark");
  assert.equal(composeAnswer("", " hi "), "hi");
  assert.equal(
    composeAnswer("Raise budget", "wait for tomorrow"),
    "wait for tomorrow",
  );
  assert.equal(composeAnswer("dark", "   "), "dark");
  assert.equal(composeAnswer("", ""), "");
});

const choice = (pick, note = "") => ({
  pick,
  preselected: "Raise budget",
  note,
});

test("the preselected first option shows picked and a click on Answer sends it", async () => {
  const { shownPick, clickAnswer } = await load();
  assert.equal(shownPick(choice(undefined)), "Raise budget");
  assert.equal(clickAnswer(choice(undefined)), "Raise budget");
  // Typed text hides the pick and is the whole answer.
  assert.equal(shownPick(choice(undefined, "wait")), "");
  assert.equal(clickAnswer(choice(undefined, "wait")), "wait");
  assert.equal(clickAnswer(choice("Stop", "wait")), "wait");
  // An unpicked question has nothing to send until text is typed.
  assert.equal(clickAnswer(choice("")), "");
});

test("togglePick: clicking the picked option unpicks it, the preselected one too", async () => {
  const { togglePick } = await load();
  assert.equal(togglePick(choice(undefined), "Raise budget"), "");
  assert.equal(togglePick(choice(undefined), "Stop"), "Stop");
  assert.equal(togglePick(choice("Stop"), "Stop"), "");
  assert.equal(togglePick(choice(""), "Raise budget"), "Raise budget");
  // While text replaces the pick, a click picks rather than unpicks.
  assert.equal(togglePick(choice("Stop", "note"), "Stop"), "Stop");
});

test("enterAnswer never sends the preselection and waits out the arm delay", async () => {
  const { enterAnswer, ENTER_ARM_MS } = await load();
  const shownAt = 10_000;
  const later = shownAt + ENTER_ARM_MS;
  // Opening the Inbox (or the bubble) and pressing Enter answers nothing.
  assert.equal(enterAnswer(choice(undefined), shownAt, later), "");
  // An explicit pick (1-N or a click) or typed text for this item is sent.
  assert.equal(enterAnswer(choice("Stop"), shownAt, later), "Stop");
  assert.equal(
    enterAnswer(choice(undefined, " wait "), shownAt, later),
    "wait",
  );
  assert.equal(enterAnswer(choice("Stop", "wait"), shownAt, later), "wait");
  assert.equal(enterAnswer(choice(""), shownAt, later), "");
  // A held or second Enter right after the selection moved is ignored,
  // even for an item that has a choice.
  assert.equal(enterAnswer(choice("Stop"), shownAt, later - 1), "");
  assert.equal(enterAnswer(choice(undefined, "wait"), shownAt, shownAt), "");
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
