const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  MAX_ANSWER_CHARS,
  noticeKey,
  toNotice,
  validateAnswer,
  rerunNotice,
  createOrchestratorNotices,
} = require("../electron/orchestrator-notices.cjs");
const { orchestratorNotice } = require("../electron/orchestrator.cjs");
const { validateNotice } = require("../electron/extensions/notices.cjs");

const TASK = "0f8fad5b-d9cb-469f-a165-70867728950e";
const base = (kind, extra = {}) => ({
  taskId: TASK,
  repo: "/work/evidence-repo",
  kind,
  title: "Title",
  body: "Body",
  focus: kind === "input" ? "question" : "summary",
  ...extra,
});

test("a needs-input notice maps to header, choices, a reply field and an open action", () => {
  const notice = toNotice(
    base("input", {
      options: ["Yes", "No"],
      askedBy: "verify",
      at: 1000,
      repoName: "evidence-repo",
    }),
  );
  assert.deepEqual(notice, {
    key: `input:${TASK}`,
    kind: "input",
    label: "Needs you",
    title: "Title",
    body: "Body",
    actions: [{ id: "open", label: "Open task" }],
    header: "evidence-repo",
    at: 1000,
    reply: true,
    choices: ["Yes", "No"],
    meta: ["Title", "asked by verify"],
  });
});

test("done maps to a meta line and Land only when it can land", () => {
  const landable = toNotice(
    base("done", { costUsd: 0.31, verdict: "PASS", canLand: true }),
  );
  assert.equal(landable.kind, "done");
  assert.deepEqual(landable.meta, ["$0.31", "review PASS", "not landed"]);
  assert.deepEqual(
    landable.actions.map((action) => action.id),
    ["land", "open"],
  );
  assert.equal(landable.actions[0].emphasis, "primary");
  assert.equal(landable.header, "evidence-repo");
  const landed = toNotice(base("done", { costUsd: 1, landed: true }));
  assert.deepEqual(landed.meta, ["$1.00", "landed"]);
  assert.deepEqual(
    landed.actions.map((action) => action.id),
    ["open"],
  );
  assert.equal(toNotice(base("done")).meta, undefined);
});

test("failed and stopped offer Run again; landing is informational", () => {
  for (const kind of ["failed", "stopped"]) {
    const notice = toNotice(base(kind));
    assert.equal(notice.kind, "failed");
    assert.deepEqual(
      notice.actions.map((action) => action.id),
      ["rerun", "open"],
    );
  }
  assert.equal(toNotice(base("stopped")).label, "Stopped");
  const landing = toNotice(base("landing"));
  assert.equal(landing.kind, "info");
  assert.equal(landing.label, "Landing");
});

test("keys: one per open question, one per kind and body otherwise, per host", () => {
  assert.equal(noticeKey(base("input")), `input:${TASK}`);
  assert.equal(
    noticeKey(base("input", { host: "ssh:box", body: "other" })),
    `input:ssh:box/${TASK}`,
  );
  assert.equal(noticeKey(base("done")), `done:${TASK}:Body`);
  assert.notEqual(
    noticeKey(base("done")),
    noticeKey(base("done", { body: "x" })),
  );
});

test("every notice orchestratorNotice builds passes the generic validation", () => {
  const tasks = [
    {
      id: TASK,
      title: "T",
      repo: "/r/x",
      status: "waiting",
      question: { text: "Q?", options: ["a", "b"], askedBy: "verify" },
    },
    {
      id: TASK,
      title: "T",
      repo: "/r/x",
      status: "done",
      costUsd: 1,
      baseRef: "main",
    },
    { id: TASK, title: "T", repo: "/r/x", status: "failed", costUsd: 1 },
    { id: TASK, title: "T", repo: "/r/x", status: "landing" },
    { id: TASK, title: "T", repo: "/r/x", status: "stopped", host: "ssh:box" },
  ];
  for (const task of tasks)
    assert.doesNotThrow(() =>
      validateNotice(toNotice(orchestratorNotice(task))),
    );
});

const waiting = {
  id: "a",
  status: "waiting",
  question: { text: "q", options: [] },
};
const queue = [{ kind: "input", taskId: "a" }];

test("validateAnswer accepts a tracked waiting task and returns the trimmed text", () => {
  assert.equal(validateAnswer("a", "  yes  ", waiting, queue), "yes");
  assert.equal(
    validateAnswer("a", "x".repeat(MAX_ANSWER_CHARS), waiting, queue).length,
    MAX_ANSWER_CHARS,
  );
});

test("validateAnswer rejects a task that is not waiting and every bad input", () => {
  const bad = (...args) => assert.throws(() => validateAnswer(...args));
  bad("a", "yes", { ...waiting, status: "running" }, queue);
  bad("a", "yes", { id: "a", status: "waiting" }, queue);
  bad("a", "yes", null, queue);
  bad("a", "yes", { ...waiting, id: "other" }, queue);
  bad("b", "yes", { ...waiting, id: "b" }, queue);
  bad("a", "yes", waiting, [{ kind: "done", taskId: "a" }]);
  bad("a", "yes", waiting, []);
  bad("a", "yes", waiting);
  bad("a", "", waiting, queue);
  bad("a", "   \n ", waiting, queue);
  bad("a", 42, waiting, queue);
  bad("a", "x".repeat(MAX_ANSWER_CHARS + 1), waiting, queue);
  bad(7, "yes", waiting, queue);
});

test("rerunNotice checks the id shape and the kind", () => {
  assert.equal(
    rerunNotice("not-a-uuid", [{ kind: "failed", taskId: "not-a-uuid" }]),
    null,
  );
  const ok = [{ kind: "stopped", taskId: TASK }];
  assert.equal(rerunNotice(TASK, ok), ok[0]);
  assert.equal(rerunNotice(TASK.toUpperCase(), ok), null);
  assert.equal(rerunNotice(TASK, [{ kind: "done", taskId: TASK }]), null);
});

function setup(task = waiting) {
  const log = [];
  let handler;
  const calls = [];
  const adapter = createOrchestratorNotices({
    notices: {
      register: (id, onAction) => {
        log.push(["register", id]);
        handler = onAction;
      },
      publish: (id, notice) => log.push(["publish", id, notice.key]),
      retract: (id, key) => log.push(["retract", id, key]),
    },
    getService: () => ({
      call: async (method, params, host) => {
        calls.push(host ? [method, params, host] : [method, params]);
        return method === "task.get" ? task : {};
      },
    }),
    showWindow: () => log.push(["show"]),
    send: (channel, value) => log.push(["send", channel, value]),
  });
  return { adapter, log, calls, act: (...args) => handler(...args) };
}

test("answering a waiting task calls the service and confirms", async () => {
  const id = "a1111111-1111-4111-8111-111111111111";
  const { adapter, calls, act } = setup({ ...waiting, id });
  adapter.publish(base("input", { taskId: id, host: "ssh:box" }));
  const message = await act(`input:ssh:box/${id}`, "reply", " yes ");
  assert.match(message, /^Answered/);
  assert.deepEqual(calls, [
    ["task.get", { id }, "ssh:box"],
    ["task.answer", { id, answer: "yes" }, "ssh:box"],
  ]);
  await assert.rejects(act(`input:ssh:box/${id}`, "reply", "again"), /gone/);
});

test("answering a task that is not waiting is refused without an answer call", async () => {
  const { adapter, calls, act } = setup({
    ...waiting,
    id: TASK,
    status: "running",
  });
  adapter.publish(base("input"));
  await assert.rejects(act(`input:${TASK}`, "reply", "yes"), /not waiting/);
  assert.deepEqual(calls, [["task.get", { id: TASK }]]);
});

test("only an input notice takes an answer", async () => {
  const { adapter, calls, act } = setup();
  adapter.publish(base("done", { canLand: true }));
  await assert.rejects(
    act(`done:${TASK}:Body`, "answer", "yes"),
    /No open question/,
  );
  assert.equal(calls.filter(([method]) => method === "task.answer").length, 0);
});

test("Land needs a landable done notice; Run again a failed or stopped one", async () => {
  const land = setup();
  land.adapter.publish(base("done", { canLand: true, host: "ssh:box" }));
  assert.equal(await land.act(`done:ssh:box/${TASK}:Body`, "land"), undefined);
  assert.deepEqual(land.calls, [["task.land", { id: TASK }, "ssh:box"]]);

  const blocked = setup();
  blocked.adapter.publish(base("done"));
  await assert.rejects(blocked.act(`done:${TASK}:Body`, "land"), /gone/);
  await assert.rejects(blocked.act(`done:${TASK}:Body`, "rerun"), /gone/);
  assert.deepEqual(blocked.calls, []);

  for (const kind of ["failed", "stopped"]) {
    const run = setup();
    run.adapter.publish(base(kind));
    assert.equal(await run.act(`${kind}:${TASK}:Body`, "rerun"), undefined);
    assert.deepEqual(run.calls, [["task.start", { id: TASK }]]);
  }
});

test("a notice that was never published or an unknown action is refused", async () => {
  const { adapter, calls, act } = setup();
  await assert.rejects(act("done:nope", "land"), /gone/);
  adapter.publish(base("failed"));
  await assert.rejects(act(`failed:${TASK}:Body`, "explode"), /Unknown action/);
  assert.deepEqual(calls, []);
});

test("open shows the window and asks the renderer to open the task", async () => {
  const { adapter, log, act } = setup();
  adapter.publish(base("input", { host: "ssh:box" }));
  await act(`input:ssh:box/${TASK}`, "open");
  assert.deepEqual(log.slice(-2), [
    ["show"],
    [
      "send",
      "orchestrator-open",
      {
        taskId: TASK,
        repo: "/work/evidence-repo",
        focus: "question",
        host: "ssh:box",
      },
    ],
  ]);
});

test("a service that is not running refuses an action", async () => {
  let fn;
  const adapter = createOrchestratorNotices({
    notices: {
      register: (_id, onAction) => (fn = onAction),
      publish() {},
      retract() {},
    },
    getService: () => null,
    showWindow() {},
    send() {},
  });
  adapter.publish(base("failed"));
  await assert.rejects(fn(`failed:${TASK}:Body`, "rerun"), /not running/);
});

test("a task that leaves waiting retracts only its own open question", () => {
  const { adapter, log } = setup();
  const other = "b2222222-2222-4222-8222-222222222222";
  adapter.publish(base("input"));
  adapter.publish(base("input", { taskId: other }));
  adapter.publish(base("done"));
  log.length = 0;
  adapter.onTask({ id: TASK, status: "waiting" });
  assert.deepEqual(log, []);
  adapter.onTask({ id: TASK, status: "running" });
  assert.deepEqual(log, [["retract", "builtin.orchestrator", `input:${TASK}`]]);
});

test("publishing registers one source and a bad notice is dropped quietly", () => {
  const { adapter, log } = setup();
  assert.deepEqual(log[0], ["register", "builtin.orchestrator"]);
  adapter.publish(base("done"));
  assert.deepEqual(log[1], [
    "publish",
    "builtin.orchestrator",
    `done:${TASK}:Body`,
  ]);
});
