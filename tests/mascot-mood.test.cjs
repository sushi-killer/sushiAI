const { test } = require("node:test");
const assert = require("node:assert/strict");
const { mascotMood } = require("../src/mascot/mood.ts");

test("maps every notice kind to a mood", () => {
  assert.equal(mascotMood({ kind: "input" }, false), "needs-you");
  assert.equal(mascotMood({ kind: "done" }, false), "done");
  assert.equal(mascotMood({ kind: "failed" }, false), "failed");
  assert.equal(mascotMood({ kind: "info" }, false), "idle");
  assert.equal(mascotMood({ kind: "info", sticky: true }, false), "idle");
});

test("an input notice listens while the owner types", () => {
  assert.equal(mascotMood({ kind: "input" }, true), "listening");
  assert.equal(mascotMood({ kind: "done" }, true), "done");
});

test("a confirmed notice means the source carries on", () => {
  const confirmed = { kind: "input", confirmed: "Sent" };
  assert.equal(mascotMood(confirmed, false), "working");
  assert.equal(mascotMood(confirmed, true), "working");
});

test("typing belongs to one notice and never leaks to the next", () => {
  const { isTyping } = require("../src/mascot/mood.ts");
  const first = { id: "input:t1", kind: "input" };
  let state = { id: first.id, on: true };
  assert.equal(isTyping(state, first), true);
  assert.equal(isTyping(state, { id: "input:t2" }), false);
  assert.equal(isTyping(state, { ...first, confirmed: "Sent" }), false);
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
