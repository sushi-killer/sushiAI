const { test } = require("node:test");
const assert = require("node:assert/strict");

const model = import("../src/orchestrator/planModel.ts");

function task(overrides = {}) {
  return {
    id: "t1",
    title: "Draft",
    goal: "goal",
    criteria: [],
    verify: [],
    repo: "/r",
    worktree: "/w",
    branch: "b",
    baseSha: "a",
    status: "stopped",
    tier: "standard",
    decisions: [],
    attempts: [],
    costUsd: 0,
    createdAt: 1000,
    updatedAt: 1000,
    ...overrides,
  };
}

test("drafts outside the backlog are NEXT, oldest first", async () => {
  const { planModel } = await model;
  const m = planModel([
    task({ id: "b", createdAt: 2000 }),
    task({ id: "a", createdAt: 1000 }),
    task({ id: "run", status: "running" }),
  ]);
  assert.deepEqual(
    m.next.map((i) => i.task.id),
    ["a", "b"],
  );
  assert.equal(m.later.length, 0);
});

test("the backlog bucket and order split NEXT and LATER, backlog first", async () => {
  const { planModel } = await model;
  const m = planModel([
    task({ id: "a", backlog: { bucket: "later", order: 1 } }),
    task({ id: "b", backlog: { bucket: "next", order: 2 }, createdAt: 1 }),
    task({ id: "c", backlog: { bucket: "next", order: 1 }, createdAt: 9 }),
    task({ id: "loose", createdAt: 0 }),
  ]);
  assert.deepEqual(
    m.next.map((i) => i.task.id),
    ["c", "b", "loose"],
  );
  assert.deepEqual(
    m.later.map((i) => i.task.id),
    ["a"],
  );
});

test("a task that already ran an attempt is not a draft", async () => {
  const { planModel, unstartedDraft } = await model;
  const planning = task({
    id: "planning",
    status: "queued",
    attempts: [{ n: 1, stage: "plan", status: "running" }],
  });
  const parked = task({
    id: "parked",
    backlog: { bucket: "next", order: 0 },
    attempts: [{ n: 1, stage: "plan", status: "passed" }],
  });
  assert.equal(unstartedDraft(planning), false);
  assert.equal(unstartedDraft(parked), true);
  assert.deepEqual(
    planModel([planning, parked, task({ id: "new" })]).next.map(
      (i) => i.task.id,
    ),
    ["parked", "new"],
  );
});

test("backlog moves renumber the bucket and send only what changed", async () => {
  const { planModel, backlogMoves } = await model;
  const m = planModel([
    task({ id: "a", backlog: { bucket: "next", order: 0 } }),
    task({ id: "b", backlog: { bucket: "next", order: 1 } }),
    task({ id: "loose", createdAt: 5 }),
  ]);
  assert.deepEqual(backlogMoves(m.next, 1, -1, "next"), [
    { id: "b", order: 0 },
    { id: "a", order: 1 },
  ]);
  // Moving the loose draft up parks it in the bucket.
  assert.deepEqual(backlogMoves(m.next, 2, -1, "next"), [
    { id: "loose", order: 1 },
    { id: "b", order: 2 },
  ]);
  assert.deepEqual(backlogMoves(m.next, 0, -1, "next"), []);
  assert.deepEqual(backlogMoves(m.next, 2, 1, "next"), []);
});

test("the autopilot's next is the first ready NEXT draft in the backlog", async () => {
  const { planModel, autopilotNext } = await model;
  const tasks = [
    task({ id: "loose", title: "Loose", createdAt: 0 }),
    task({
      id: "blocked",
      title: "Blocked",
      backlog: { bucket: "next", order: 0 },
      dependsOn: ["loose"],
    }),
    task({
      id: "ready",
      title: "Ready",
      backlog: { bucket: "next", order: 1 },
    }),
    task({ id: "later", backlog: { bucket: "later", order: 0 } }),
  ];
  assert.equal(autopilotNext(planModel(tasks)).title, "Ready");
  assert.equal(autopilotNext(planModel([task({ id: "x" })])), undefined);
});

test("split drafts group under their parent with after/waits", async () => {
  const { planModel, readyDrafts } = await model;
  const tasks = [
    task({ id: "p", title: "Parent" }),
    task({ id: "c1", parent: "p", title: "One", createdAt: 1 }),
    task({
      id: "c2",
      parent: "p",
      title: "Two",
      createdAt: 2,
      dependsOn: ["c1"],
    }),
    task({ id: "s", title: "Single", dependsOn: ["gone"] }),
    task({ id: "d", title: "Done", status: "done" }),
  ];
  const m = planModel(tasks);
  assert.deepEqual(
    m.next.map((i) => i.task.id),
    ["p", "s"],
  );
  const [one, two] = m.next[0].children;
  assert.equal(one.waits, false);
  assert.deepEqual(two.after, ["One"]);
  assert.equal(two.waits, true);
  // "s" waits on an unknown (not done) task, so only the parent is ready.
  assert.deepEqual(
    readyDrafts(m).map((t) => t.id),
    ["p"],
  );
});

test("meta pluralises criteria", async () => {
  const { draftMeta } = await model;
  assert.equal(
    draftMeta(task({ tier: "hard", criteria: ["a"] })),
    "hard · 1 criterion",
  );
  assert.equal(
    draftMeta(task({ criteria: ["a", "b"] })),
    "standard · 2 criteria",
  );
});

test("draftCreateParams resolves dependencies and parks an unstarted draft", async () => {
  const { draftCreateParams } = await model;
  const tasks = [task({ id: "t9", title: "Lease table" })];
  const draft = {
    title: "T",
    goal: "",
    criteria: ["c"],
    dependsOn: ["lease table", "unknown"],
  };
  assert.deepEqual(draftCreateParams(draft, tasks, false), {
    title: "T",
    goal: "T",
    criteria: ["c"],
    dependsOn: ["t9"],
    source: "brainstorm",
    start: false,
    backlog: { bucket: "next" },
  });
  const started = draftCreateParams(draft, tasks, true);
  assert.equal(started.start, true);
  assert.equal("backlog" in started, false);
});

test("planOnly keeps a stopped zero-attempt task that needs the owner on the rail", async () => {
  const { planOnly } = await model;
  const draft = task({ id: "draft" });
  const parked = task({ id: "parked", backlog: { bucket: "next", order: 0 } });
  const halted = task({
    id: "halted",
    status: "stopped",
    decisions: ["Orchestrator: stopped - the brief is unclear"],
  });
  const ownerStopped = task({
    id: "owner-stopped",
    status: "stopped",
    decisions: ["Owner: stop"],
  });
  const only = planOnly([draft, parked, halted, ownerStopped]);
  assert.deepEqual([...only].map((t) => t.id).sort(), [
    "draft",
    "owner-stopped",
    "parked",
  ]);
});
