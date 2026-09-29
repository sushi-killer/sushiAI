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

test("without daemon buckets every draft is NEXT, oldest first", async () => {
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
  assert.equal(m.bucketed, false);
});

test("daemon bucket and order split NEXT and LATER", async () => {
  const { planModel } = await model;
  const m = planModel([
    task({ id: "a", bucket: "later", order: 1 }),
    task({ id: "b", bucket: "next", order: 2, createdAt: 1 }),
    task({ id: "c", bucket: "next", order: 1, createdAt: 9 }),
  ]);
  assert.deepEqual(
    m.next.map((i) => i.task.id),
    ["c", "b"],
  );
  assert.deepEqual(
    m.later.map((i) => i.task.id),
    ["a"],
  );
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

test("autopilot is read defensively", async () => {
  const { autopilotOf } = await model;
  assert.equal(autopilotOf(null), null);
  assert.equal(autopilotOf({}), null);
  assert.equal(autopilotOf({ autopilot: true }), true);
  assert.equal(autopilotOf({ autopilot: { enabled: false } }), false);
});

test("parseDraftBlock reads the last valid sushi-plan block", async () => {
  const { parseDraftBlock, stripDraftBlock } = await model;
  const text =
    'Here:\n```sushi-plan\n{"title":"A","criteria":["x",{"text":"y","met":true}],"tier":"hard","dependsOn":["Z"]}\n```\nok\n```sushi-plan\n{bad\n```';
  const draft = parseDraftBlock(text);
  assert.equal(draft.title, "A");
  assert.deepEqual(draft.criteria, [
    { text: "x", met: false },
    { text: "y", met: true },
  ]);
  assert.equal(draft.tier, "hard");
  assert.deepEqual(draft.dependsOn, ["Z"]);
  assert.equal(parseDraftBlock("no block"), null);
  assert.equal(parseDraftBlock('```sushi-plan\n{"goal":"x"}\n```'), null);
  assert.equal(stripDraftBlock(text).includes("sushi-plan"), false);
});

test("a structured thread draft wins over parsing chat text", async () => {
  const { draftOfThread, optionsOf } = await model;
  const messages = [
    { role: "assistant", text: '```sushi-plan\n{"title":"Parsed"}\n```' },
  ];
  assert.equal(draftOfThread({ messages }).title, "Parsed");
  assert.equal(
    draftOfThread({ draft: { title: "Live" }, messages }).title,
    "Live",
  );
  assert.equal(draftOfThread({ messages: [] }), null);
  assert.deepEqual(
    optionsOf({ role: "assistant", text: "", options: ["a", 1] }),
    ["a"],
  );
  assert.deepEqual(optionsOf({ role: "assistant", text: "" }), []);
});

test("draftCreateParams resolves dependencies and honours start", async () => {
  const { draftCreateParams } = await model;
  const tasks = [task({ id: "t9", title: "Lease table" })];
  const draft = {
    title: "T",
    goal: "",
    criteria: [{ text: "c", met: false }],
    dependsOn: ["lease table", "unknown"],
  };
  assert.deepEqual(draftCreateParams(draft, tasks, false), {
    title: "T",
    goal: "T",
    criteria: ["c"],
    dependsOn: ["t9"],
    start: false,
  });
  assert.equal(draftCreateParams(draft, tasks, true).start, true);
});
