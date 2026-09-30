const { test } = require("node:test");
const assert = require("node:assert/strict");

const model = import("../src/orchestrator/homeModel.ts");

function task(overrides = {}) {
  return {
    id: "t1",
    title: "T",
    goal: "g",
    criteria: [],
    verify: [],
    repo: "/r",
    worktree: "/w",
    branch: "b",
    baseSha: "a",
    status: "running",
    tier: "standard",
    decisions: [],
    attempts: [],
    costUsd: 0,
    createdAt: 1000,
    updatedAt: 1000,
    ...overrides,
  };
}
const fail = (n, signature, kind = "verify") => ({
  n,
  stage: "implement",
  status: "failed",
  startedAt: 0,
  failure: { kind, detail: "", signature },
});

test("greeting and lines pluralise", async () => {
  const m = await model;
  assert.equal(m.greeting(0), "Nothing needs you");
  assert.equal(m.greeting(1), "1 thing needs you");
  assert.equal(m.greeting(2), "2 things need you");
  assert.equal(
    m.subLine(4, 1.21),
    "4 tasks active · $1.21 today · orchd on Local",
  );
  assert.equal(m.subLine(1, null), "1 task active · orchd on Local");
  assert.equal(
    m.subLine(2, 0.5, "lab"),
    "orchd on lab keeps running when your Mac sleeps",
  );
  assert.equal(m.clearLine(2, 3), "2 tasks running, 3 landed today.");
  assert.equal(m.clearLine(1, 0), "1 task running, 0 landed today.");
});

test("question meta names who asked and when, as far as orchd recorded it", async () => {
  const m = await model;
  const t = task({
    status: "waiting",
    attempts: [{ n: 1, stage: "implement" }],
    question: { text: "?", options: [], kind: "agent_question" },
  });
  assert.equal(m.questionMeta("sushiai", t, 4, 0), "sushiai · attempt 1/4");
  t.question.askedBy = "verify";
  t.question.askedAt = 0;
  assert.equal(
    m.questionMeta("sushiai", t, 4, 2 * 60_000),
    "sushiai · attempt 1/4 · asked by verify · 2m ago",
  );
});

test("same-failure note needs repeated identical signatures", async () => {
  const m = await model;
  assert.equal(m.sameFailureNote(task({ attempts: [fail(1, "a")] })), "");
  assert.equal(
    m.sameFailureNote(task({ attempts: [fail(1, "a"), fail(2, "b")] })),
    "",
  );
  assert.match(
    m.sameFailureNote(task({ attempts: [fail(1, "a"), fail(2, "a")] })),
    /same test failed every attempt/,
  );
  assert.match(
    m.sameFailureNote(
      task({ attempts: [fail(1, "a", "stall"), fail(2, "a", "stall")] }),
    ),
    /same failure every attempt/,
  );
});

test("running meta shows stage and minutes, or queued", async () => {
  const m = await model;
  const running = task({
    attempts: [{ n: 1, stage: "implement", status: "running", startedAt: 0 }],
  });
  assert.equal(m.runningMeta(running, 6 * 60 * 1000), "implement · 6m");
  assert.equal(m.runningMeta(task({ status: "queued" }), 1), "queued");
});

test("Run with a note writes a chat instruction and never calls task.amend", async () => {
  const { runNoteMessage } = await model;
  assert.equal(
    runNoteMessage({ id: "t9", title: "Fix export" }, "  use the CSV path "),
    "Run task Fix export (t9) again with this note from the owner: use the CSV path",
  );
  const source = require("node:fs").readFileSync(
    require("node:path").join(__dirname, "../src/orchestrator/HomeView.tsx"),
    "utf8",
  );
  assert.ok(
    !/taskAmend|task\.amend/.test(source.replace(/\/\*[\s\S]*?\*\//g, "")),
  );
  assert.match(source, /onSend\(runNoteMessage\(/);
});
