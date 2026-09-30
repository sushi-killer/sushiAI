const { test } = require("node:test");
const assert = require("node:assert/strict");
const { mascotMood } = require("../src/mascot/mood.ts");

test("maps every notice kind to a mood", () => {
  assert.equal(mascotMood({ kind: "input" }, false), "needs-you");
  assert.equal(mascotMood({ kind: "done" }, false), "done");
  assert.equal(mascotMood({ kind: "failed" }, false), "failed");
  assert.equal(mascotMood({ kind: "stopped" }, false), "failed");
  assert.equal(mascotMood({ kind: "landing" }, false), "idle");
  assert.equal(mascotMood({ kind: "core-update" }, false), "idle");
});

test("an input notice listens while the owner types", () => {
  assert.equal(mascotMood({ kind: "input" }, true), "listening");
  assert.equal(mascotMood({ kind: "done" }, true), "done");
});

test("an answered notice means the task carries on", () => {
  assert.equal(mascotMood({ kind: "input", answered: true }, false), "working");
  assert.equal(mascotMood({ kind: "input", answered: true }, true), "working");
});

test("typing belongs to one notice and never leaks to the next", () => {
  const { isTyping } = require("../src/mascot/mood.ts");
  const first = { id: "input:t1", kind: "input" };
  let state = { id: first.id, on: true };
  assert.equal(isTyping(state, first), true);
  assert.equal(isTyping(state, { id: "input:t2" }), false);
  assert.equal(isTyping(state, { ...first, answered: true }), false);
  // The field unmounts on answer (its cleanup reports off); a follow-up
  // question for the same task then starts as needs-you.
  state = { id: first.id, on: false };
  assert.equal(mascotMood(first, isTyping(state, first)), "needs-you");
  // Even a stale "on" from another notice cannot make it listen.
  assert.equal(
    mascotMood(first, isTyping({ id: "old", on: true }, first)),
    "needs-you",
  );
});
