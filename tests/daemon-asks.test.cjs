const { test } = require("node:test");
const assert = require("node:assert/strict");

// Asks live in the session records of the one daemon feed (`useDaemon`).
const library = import("../src/daemonSessions.ts");

const ask = (askId, session = "s1", extra = {}) => ({
  askId,
  session,
  tool: "Bash",
  input: { command: "ls -la" },
  ...extra,
});
const session = (id, extra = {}) => ({
  id,
  cmd: ["claude"],
  cwd: "/w",
  status: "running",
  cols: 80,
  rows: 24,
  ...extra,
});
const event = (method, params, host = "local", generation = 1) => ({
  method,
  params,
  host,
  generation,
});
// A host with sessions s1 and s2 listed.
const listed = async () => {
  const { applySessionList, emptyHost } = await library;
  return applySessionList(emptyHost(), [session("s1"), session("s2")]);
};
const hostsOf = (host, name = "local") => ({ [name]: host });

test("a session.ask opens an ask on its session and askClosed removes it", async () => {
  const { applyDaemonEvent, asksOf } = await library;
  let host = await listed();
  host = applyDaemonEvent(host, event("session.ask", ask("a1")));
  host = applyDaemonEvent(host, event("session.ask", ask("a2", "s2")));
  assert.deepEqual(
    asksOf(hostsOf(host), "local", "s1").map((a) => [a.askId, a.host]),
    [["a1", "local"]],
  );
  assert.deepEqual(asksOf(hostsOf(host), "box", "s1"), [], "another host");
  // Decided in the terminal: the daemon sends askClosed with decided.
  host = applyDaemonEvent(
    host,
    event("session.askClosed", { id: "s1", askId: "a1", decided: true }),
  );
  assert.deepEqual(asksOf(hostsOf(host), "local", "s1"), []);
  assert.equal(asksOf(hostsOf(host), "local", "s2").length, 1);
});

test("an unknown askClosed, an ask for an unknown session and other events change nothing", async () => {
  const { applyDaemonEvent } = await library;
  const host = await listed();
  assert.equal(
    applyDaemonEvent(host, event("session.askClosed", { askId: "zz" })),
    host,
  );
  assert.equal(
    applyDaemonEvent(host, event("session.ask", ask("a1", "nobody"))),
    host,
  );
  assert.equal(applyDaemonEvent(host, event("session.other", {})), host);
});

test("a repeated ask is not duplicated", async () => {
  const { applyDaemonEvent, asksOf } = await library;
  let host = await listed();
  host = applyDaemonEvent(host, event("session.ask", ask("a1")));
  host = applyDaemonEvent(host, event("session.ask", ask("a1")));
  assert.equal(asksOf(hostsOf(host), "local", "s1").length, 1);
});

test("a full list replaces a host's asks; session events carry their own", async () => {
  const { applyDaemonEvent, applySessionList, asksOf } = await library;
  let host = await listed();
  host = applyDaemonEvent(host, event("session.ask", ask("old")));
  host = applySessionList(host, [
    session("s1", { asks: [ask("a1"), ask("a2")] }),
    session("s2"),
  ]);
  const ids = (h, id = "s1") =>
    asksOf(hostsOf(h), "local", id).map((a) => a.askId);
  assert.deepEqual(ids(host), ["a1", "a2"]);
  // session.updated for s1 with one ask left.
  host = applyDaemonEvent(
    host,
    event("session.updated", session("s1", { asks: [ask("a2")] })),
  );
  assert.deepEqual(ids(host), ["a2"]);
  host = applyDaemonEvent(host, event("session.exited", { id: "s1", code: 0 }));
  assert.deepEqual(ids(host), []);
  // A removed session takes its asks with it.
  host = applyDaemonEvent(host, event("session.ask", ask("a3", "s2")));
  host = applyDaemonEvent(host, event("session.removed", { id: "s2" }));
  assert.deepEqual(ids(host, "s2"), []);
});

test("asks that arrive while a list is in flight are replayed on top of it", async () => {
  const { emptyFeed, emptyHost, feedEvent, startList, finishList, asksOf } =
    await library;
  // The list is a snapshot from before the ask; the ask must survive it.
  let feed = emptyFeed();
  let host = { ...emptyHost(), ready: true };
  const started = startList(feed);
  feed = started.feed;
  // session.created and session.ask both arrive before the list lands.
  for (const e of [
    event("session.created", session("s1")),
    event("session.ask", ask("a1")),
    event("session.ask", ask("a2")),
    event("session.askClosed", { id: "s1", askId: "a1", decided: true }),
  ]) {
    const step = feedEvent(feed, host, e);
    feed = step.feed;
    host = step.host;
  }
  const done = finishList(feed, started.token, host, [session("s1")]);
  assert.deepEqual(
    asksOf(hostsOf(done.host), "local", "s1").map((a) => a.askId),
    ["a2"],
    "a1 was closed and a2 is still open after the list landed",
  );
});

test("Allow and Deny build the askRespond parameters", async () => {
  const { askDecision, applyDaemonEvent, applySessionList, emptyHost, asksOf } =
    await library;
  let host = applySessionList(emptyHost(), [session("s9")]);
  host = applyDaemonEvent(host, event("session.ask", ask("a1", "s9"), "box"));
  const [open] = asksOf(hostsOf(host, "box"), "box", "s9");
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
