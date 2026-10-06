const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/app/daemonAsks.ts");

const ask = (askId, session = "s1", extra = {}) => ({
  askId,
  session,
  tool: "Bash",
  input: { command: "ls -la" },
  ...extra,
});
const event = (method, params, host = "local") => ({
  method,
  params,
  host,
  generation: 1,
});

test("a session.ask opens an ask on its host and askClosed removes it", async () => {
  const { applyAskEvent, asksOf, noAsks } = await library;
  let state = applyAskEvent(noAsks, event("session.ask", ask("a1")));
  state = applyAskEvent(state, event("session.ask", ask("a2", "s2")));
  assert.deepEqual(
    asksOf(state, "local", "s1").map((a) => [a.askId, a.host]),
    [["a1", "local"]],
  );
  assert.deepEqual(asksOf(state, "box", "s1"), [], "another host has none");
  // Decided in the terminal: the daemon sends askClosed with decided.
  state = applyAskEvent(
    state,
    event("session.askClosed", { id: "s1", askId: "a1", decided: true }),
  );
  assert.deepEqual(asksOf(state, "local", "s1"), []);
  assert.equal(asksOf(state, "local", "s2").length, 1);
});

test("an unknown askClosed and a repeated ask change nothing", async () => {
  const { applyAskEvent, noAsks } = await library;
  assert.equal(
    applyAskEvent(noAsks, event("session.askClosed", { askId: "zz" })),
    noAsks,
  );
  const once = applyAskEvent(noAsks, event("session.ask", ask("a1")));
  assert.notEqual(once, noAsks);
  assert.equal(applyAskEvent(once, event("session.other", {})), once);
});

test("a full list replaces a host's asks; session events carry their own", async () => {
  const { applyAskList, applyAskEvent, asksOf, noAsks } = await library;
  let state = applyAskEvent(noAsks, event("session.ask", ask("old")));
  state = applyAskList(state, "local", [
    { id: "s1", asks: [ask("a1"), ask("a2")] },
    { id: "s2" },
  ]);
  assert.deepEqual(
    asksOf(state, "local", "s1").map((a) => a.askId),
    ["a1", "a2"],
  );
  // session.updated for s1 with one ask left.
  state = applyAskEvent(
    state,
    event("session.updated", { id: "s1", asks: [ask("a2")] }),
  );
  assert.deepEqual(
    asksOf(state, "local", "s1").map((a) => a.askId),
    ["a2"],
  );
  state = applyAskEvent(state, event("session.exited", { id: "s1", code: 0 }));
  assert.deepEqual(asksOf(state, "local", "s1"), []);
});

test("Allow and Deny build the askRespond parameters", async () => {
  const { askDecision, applyAskEvent, asksOf, noAsks } = await library;
  const state = applyAskEvent(
    noAsks,
    event("session.ask", ask("a1", "s9"), "box"),
  );
  const [open] = asksOf(state, "box", "s9");
  assert.deepEqual(askDecision(open, "allow"), [
    "box",
    { sessionId: "s9", askId: "a1", decision: "allow" },
  ]);
  assert.deepEqual(askDecision(open, "deny")[1].decision, "deny");
});

test("summarizeAskInput gives one short line", async () => {
  const { summarizeAskInput } = await library;
  assert.equal(summarizeAskInput({ command: "ls\n  -la" }), "ls -la");
  assert.equal(
    summarizeAskInput({ file_path: "/w/a.md", content: "x" }),
    "/w/a.md",
  );
  assert.equal(summarizeAskInput({ other: 1 }), '{"other":1}');
  assert.equal(summarizeAskInput({}), "");
  assert.equal(summarizeAskInput(null), "");
  const long = summarizeAskInput({ command: "x".repeat(500) });
  assert.equal(long.length, 140);
  assert.ok(long.endsWith("…"));
});
