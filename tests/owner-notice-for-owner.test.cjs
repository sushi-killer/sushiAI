// A waiting task reaches the owner (notice, Dock badge, Inbox, counts) only
// once orchd hands its question over (`question.forOwner`); done, failed,
// landing and engine-stopped tasks keep their own rules.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { OrchestratorService } = require("../electron/orchestrator.cjs");
const { fakeDaemonManager } = require("./helpers/fake-daemon-manager.cjs");

const loadAttention = () => import("../src/orchestrator/ownerAttention.ts");

/** A service whose notices land in the returned list. */
function noticeService() {
  const notices = [];
  const service = new OrchestratorService({
    host: "local",
    getManager: () => fakeDaemonManager(),
    getConnections: () => ({ get: () => null, list: () => [] }),
    send: () => {},
    notify: (notice) => notices.push(notice),
  });
  const push = (task) => service.handleEvent({ event: "task", task });
  return { notices, push };
}

const waiting = (question = {}, extra = {}) => ({
  id: "t1",
  title: "Pick one",
  repo: "/repo",
  status: "waiting",
  archived: false,
  decisions: [],
  attempts: [],
  updatedAt: 1,
  question: {
    text: "Which way?",
    options: ["a", "b"],
    kind: "agent_question",
    askedAt: 1000,
    ...question,
  },
  ...extra,
});

test("a waiting task without forOwner raises no notice, held or not", () => {
  const { notices, push } = noticeService();
  // Waiting before any hold: the daemon's first broadcast.
  push(waiting());
  // Held by the overseer.
  push(waiting({ heldForOverseer: true, heldAt: Date.now() }));
  // A hold older than ten minutes is no hand-over either.
  push(waiting({ heldForOverseer: true, heldAt: Date.now() - 11 * 60 * 1000 }));
  push(waiting({ heldForOverseer: true, heldAt: 1 }));
  push(waiting({ forOwner: false }));
  assert.equal(notices.length, 0);
});

test("a question handed to the owner notifies exactly once per task and question", () => {
  const { notices, push } = noticeService();
  push(waiting({ heldForOverseer: true, heldAt: Date.now() }));
  push(waiting({ forOwner: true }));
  // Rebroadcasts and a later reminder turn holding it again: no repeat.
  push(waiting({ forOwner: true }));
  push(waiting({ forOwner: true, heldForOverseer: true, heldAt: Date.now() }));
  push(waiting({ forOwner: true }));
  assert.equal(notices.length, 1);
  assert.deepEqual(
    [notices[0].taskId, notices[0].kind, notices[0].body],
    ["t1", "input", "Which way?"],
  );
  // A new question with the same text is a new question.
  push(waiting({ askedAt: 2000 }));
  assert.equal(notices.length, 1, "not handed over yet");
  push(waiting({ askedAt: 2000, forOwner: true }));
  assert.equal(notices.length, 2);
  // Another task with the same question notifies on its own.
  push(waiting({ forOwner: true }, { id: "t2" }));
  assert.equal(notices.length, 3);
  // A waiting subtask asks too; an archived task never does.
  push(waiting({ forOwner: true }, { id: "sub", parent: "t1" }));
  push(waiting({ forOwner: true }, { id: "old", archived: true }));
  assert.deepEqual(
    notices.map((n) => n.taskId),
    ["t1", "t1", "t2", "sub"],
  );
});

test("done, failed, landing and engine-stopped notices are unchanged", () => {
  const { notices, push } = noticeService();
  const base = {
    title: "Ship it",
    repo: "/repo",
    branch: "task/ship",
    baseRef: "main",
    archived: false,
    decisions: [],
    attempts: [],
  };
  push({ ...base, id: "d", status: "done" });
  push({ ...base, id: "d", status: "done" });
  for (const id of ["f", "l", "s"]) push({ ...base, id, status: "running" });
  push({ ...base, id: "f", status: "failed" });
  push({ ...base, id: "l", status: "landing" });
  push({
    ...base,
    id: "s",
    status: "stopped",
    decisions: ["Orchestrator: budget spent"],
  });
  // An owner stop and a subtask failure raise nothing.
  push({ ...base, id: "o", status: "running" });
  push({ ...base, id: "o", status: "stopped", decisions: ["Owner: stop"] });
  push({ ...base, id: "sub", status: "running", parent: "d" });
  push({ ...base, id: "sub", status: "failed", parent: "d" });
  assert.deepEqual(
    notices.map((n) => [n.taskId, n.kind]),
    [
      ["d", "done"],
      ["f", "failed"],
      ["l", "landing"],
      ["s", "stopped"],
    ],
  );
});

test("only a question handed to the owner counts", async () => {
  const { needsOwner, needsOwnerNow, ownerTasks } = await loadAttention();
  const tasks = [
    waiting({}, { id: "before-hold", updatedAt: 9 }),
    waiting(
      { heldForOverseer: true, heldAt: Date.now() },
      { id: "held", updatedAt: 8 },
    ),
    waiting(
      { heldForOverseer: true, heldAt: Date.now() - 11 * 60 * 1000 },
      { id: "stale-hold", updatedAt: 7 },
    ),
    waiting({ forOwner: true }, { id: "handed", updatedAt: 6 }),
    waiting(
      { forOwner: true, heldForOverseer: true, heldAt: Date.now() },
      { id: "handed-held", updatedAt: 5 },
    ),
    waiting({ forOwner: true }, { id: "archived", archived: true }),
    {
      ...waiting(),
      id: "failed",
      status: "failed",
      question: undefined,
      updatedAt: 4,
    },
    {
      ...waiting(),
      id: "landing",
      status: "landing",
      question: undefined,
      updatedAt: 3,
    },
    {
      ...waiting(),
      id: "engine-stop",
      status: "stopped",
      question: undefined,
      decisions: ["Orchestrator: budget spent"],
      updatedAt: 2,
    },
  ];
  for (const id of ["before-hold", "held", "stale-hold"]) {
    const task = tasks.find((t) => t.id === id);
    assert.equal(needsOwner(task), true, `the rail still lists ${id}`);
    assert.equal(needsOwnerNow(task), false, id);
  }
  assert.deepEqual(
    ownerTasks(tasks).map((t) => t.id),
    ["handed", "handed-held", "failed", "landing", "engine-stop"],
  );
});
