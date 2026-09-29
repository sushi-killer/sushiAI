const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  MASCOT_TIMED_MS,
  MASCOT_ANSWERED_MS,
  MASCOT_MAX_NOTICES,
  MAX_ANSWER_CHARS,
  noticeLifetimeMs,
  queueReducer,
  validateAnswer,
  mascotBounds,
} = require("../electron/mascot.cjs");

const notice = (kind, taskId, body = "b") => ({
  taskId,
  repo: "/repo",
  kind,
  title: "t",
  body,
  focus: kind === "input" ? "question" : "summary",
});
const add = (state, n, now = 1000) =>
  queueReducer(state, { type: "add", notice: n, now });
const ids = (state) => state.map((item) => `${item.kind}:${item.taskId}`);

test("the queue puts the newest notice first and caps its length", () => {
  let state = [];
  const tasks = ["a", "b", "c", "d", "e", "f", "g"];
  for (const id of tasks) state = add(state, notice("done", id));
  assert.equal(MASCOT_MAX_NOTICES, 5);
  assert.deepEqual(ids(state), [
    "done:g",
    "done:f",
    "done:e",
    "done:d",
    "done:c",
  ]);
});

test("a repeated notice (same kind, task and body) moves to the front once", () => {
  let state = [];
  for (const id of ["a", "b"]) state = add(state, notice("done", id));
  state = add(state, notice("done", "a"), 2000);
  assert.deepEqual(ids(state), ["done:a", "done:b"]);
  assert.equal(state[0].expiresAt, 2000 + MASCOT_TIMED_MS);
  state = add(state, notice("done", "a", "other body"));
  assert.equal(state.length, 3);
});

test("dismiss removes one notice by id and clear removes all", () => {
  let state = add(add([], notice("done", "a")), notice("failed", "b"));
  state = queueReducer(state, { type: "dismiss", id: state[0].id });
  assert.deepEqual(ids(state), ["done:a"]);
  assert.deepEqual(queueReducer(state, { type: "clear" }), []);
});

test("lifetimes: input waits for the owner, done and failed last 10000 ms", () => {
  assert.equal(MASCOT_TIMED_MS, 10000);
  assert.equal(noticeLifetimeMs(notice("input", "a")), null);
  assert.equal(noticeLifetimeMs(notice("done", "a")), 10000);
  assert.equal(noticeLifetimeMs(notice("failed", "a")), 10000);
  let state = add(
    add(add([], notice("input", "a")), notice("done", "b")),
    notice("failed", "c"),
  );
  assert.equal(state.find((i) => i.kind === "input").expiresAt, null);
  state = queueReducer(state, { type: "expire", now: 1000 + 9999 });
  assert.equal(state.length, 3);
  state = queueReducer(state, { type: "expire", now: 1000 + 10000 });
  assert.deepEqual(ids(state), ["input:a"]);
  assert.equal(queueReducer(state, { type: "expire", now: 1e15 }).length, 1);
});

test("a needs-input notice is dropped once its task is no longer waiting", () => {
  let state = add(
    add(add([], notice("input", "a")), notice("input", "b")),
    notice("done", "a"),
  );
  const same = queueReducer(state, {
    type: "task",
    taskId: "a",
    status: "waiting",
  });
  assert.equal(same, state);
  state = queueReducer(state, { type: "task", taskId: "a", status: "running" });
  assert.deepEqual(ids(state), ["done:a", "input:b"]);
});

test("an answered notice shows a short confirmation and survives the task leaving waiting", () => {
  let state = add([], notice("input", "a"));
  state = queueReducer(state, { type: "answered", taskId: "a", now: 5000 });
  assert.equal(state[0].answered, true);
  assert.equal(state[0].expiresAt, 5000 + MASCOT_ANSWERED_MS);
  state = queueReducer(state, { type: "task", taskId: "a", status: "running" });
  assert.equal(state.length, 1);
  state = queueReducer(state, {
    type: "expire",
    now: 5000 + MASCOT_ANSWERED_MS,
  });
  assert.deepEqual(state, []);
});

const waiting = {
  id: "a",
  status: "waiting",
  question: { text: "q", options: [] },
};
const queue = add([], notice("input", "a"));

test("validateAnswer accepts a queued waiting task and returns the trimmed text", () => {
  assert.equal(validateAnswer("a", "  yes  ", waiting, queue), "yes");
  assert.equal(
    validateAnswer("a", "x".repeat(MAX_ANSWER_CHARS), waiting, queue).length,
    MAX_ANSWER_CHARS,
  );
});

test("validateAnswer rejects every bad case", () => {
  assert.equal(MAX_ANSWER_CHARS, 2000);
  const bad = (...args) => assert.throws(() => validateAnswer(...args));
  bad("a", "yes", { ...waiting, status: "running" }, queue);
  bad("a", "yes", { id: "a", status: "waiting" }, queue);
  bad("a", "yes", null, queue);
  bad("a", "yes", { ...waiting, id: "other" }, queue);
  bad("b", "yes", { ...waiting, id: "b" }, queue);
  bad("a", "yes", waiting, add([], notice("done", "a")));
  bad("a", "yes", waiting, []);
  bad("a", "yes", waiting);
  const answered = queueReducer(queue, {
    type: "answered",
    taskId: "a",
    now: 1,
  });
  bad("a", "yes", waiting, answered);
  bad("a", "", waiting, queue);
  bad("a", "   \n ", waiting, queue);
  bad("a", 42, waiting, queue);
  bad("a", "x".repeat(MAX_ANSWER_CHARS + 1), waiting, queue);
  bad(7, "yes", waiting, queue);
});

test("mascotBounds hugs the bottom-right of the work area and grows for input", () => {
  const area = { x: 0, y: 25, width: 1440, height: 875 };
  const timed = mascotBounds(area, add([], notice("done", "a")));
  const input = mascotBounds(area, add([], notice("input", "a")));
  for (const b of [timed, input]) {
    assert.equal(b.x + b.width, 1440 - 12);
    assert.equal(b.y + b.height, 900 - 12);
  }
  assert.ok(input.height > timed.height);
});
