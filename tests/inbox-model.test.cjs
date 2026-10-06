const { test } = require("node:test");
const assert = require("node:assert/strict");

const load = () => import("../src/app/inboxModel.ts");
const task = (over = {}) => ({
  id: "t1",
  title: "T",
  repo: "/w/app",
  status: "done",
  archived: false,
  decisions: [],
  attempts: [],
  updatedAt: 1000,
  baseRef: "main",
  ...over,
});
const row = (id, group, over = {}) => ({
  workspace: { id: `w-${id}`, name: "app", panels: [], ...over.workspace },
  panel: { id, title: id, kind: "agent", agent: "claude", ...over.panel },
  group,
  since: over.since ?? 1,
});
const groups = (rows) =>
  ["blocked", "done", "working", "idle", "shells"].map((key) => ({
    key,
    label: key,
    rows: rows.filter((r) => r.group === key),
  }));

test("sessions alone fill the Inbox: blocked answers, unseen done, working and idle - shells stay out", async () => {
  const { inboxItems, needsYou } = await load();
  const items = inboxItems(
    [],
    [],
    groups([
      row("b", "blocked"),
      row("d", "done"),
      row("w", "working"),
      row("i", "idle"),
      row("s", "shells", { panel: { kind: "terminal", agent: undefined } }),
    ]),
  );
  assert.deepEqual(
    items.map((item) => [item.kind, item.key]),
    [
      ["answer", "panel:b"],
      ["panels", "panel:d"],
      ["working", "panel:w"],
      ["idle", "panel:i"],
    ],
  );
  assert.equal(items[0].title, "app · Claude Code");
  assert.equal(items.filter(needsYou).length, 2);
});

test("orchestrator answers mix into the same ANSWER group as sessions", async () => {
  const { inboxItems } = await load();
  const items = inboxItems(
    [task({ id: "q", status: "waiting", updatedAt: 5 })],
    [task({ id: "done1" })],
    groups([row("b", "blocked", { since: 9 })]),
  );
  assert.deepEqual(
    items.map((item) => [item.kind, item.source]),
    [
      ["answer", "session"],
      ["answer", "task"],
      ["land", "task"],
    ],
  );
});

test("Land N lands exactly the LAND rows a filter leaves on screen", async () => {
  const { inboxItems, inScope, landTargets } = await load();
  const all = [
    task({ id: "a", repo: "/w/app" }),
    task({ id: "b", repo: "/w/site" }),
    task({ id: "c", repo: "/w/app", title: "Other" }),
  ];
  const items = inboxItems([], all, groups([]));
  const visible = items.filter((item) =>
    inScope(item, { project: "app", host: "", query: "" }),
  );
  assert.deepEqual(
    landTargets(visible).map((t) => t.id),
    ["a", "c"],
  );
  const searched = items.filter((item) =>
    inScope(item, { project: "", host: "", query: "other" }),
  );
  assert.deepEqual(
    landTargets(searched).map((t) => t.id),
    ["c"],
  );
});

test("Clean up offers idle agents and shells apart, within the host filter", async () => {
  const { cleanupCandidates } = await load();
  const remote = { workspace: { name: "api", connection: "ssh:lab" } };
  const all = groups([
    row("idle-local", "idle"),
    row("idle-remote", "idle", remote),
    row("dev-server", "shells", { panel: { kind: "terminal" } }),
    row("blocked", "blocked"),
    row("working", "working"),
  ]);
  const local = cleanupCandidates(all, { project: "", host: "local" });
  assert.deepEqual(
    local.agents.map((r) => r.panel.id),
    ["idle-local"],
  );
  assert.deepEqual(
    local.shells.map((r) => r.panel.id),
    ["dev-server"],
  );
  const everywhere = cleanupCandidates(all, { project: "", host: "" });
  assert.deepEqual(
    everywhere.agents.map((r) => r.panel.id),
    ["idle-local", "idle-remote"],
  );
  assert.deepEqual(
    cleanupCandidates(all, { project: "api", host: "" }).shells,
    [],
  );
});

test("Inbox counts read singular for one and plural otherwise", async () => {
  const { plural } = await load();
  assert.equal(plural(1, "idle session"), "1 idle session");
  assert.equal(plural(2, "idle session"), "2 idle sessions");
  assert.equal(plural(1, "shell"), "1 shell");
  assert.equal(plural(0, "file"), "0 files");
});

test("a remote-host task is filed under its host, not Local", async () => {
  const { inboxItems } = await import("../src/app/inboxModel.ts");
  const task = {
    id: "t1",
    title: "Remote question",
    repo: "/srv/app",
    status: "waiting",
    updatedAt: 1,
    host: "ssh:lab",
    question: { text: "Which?", options: ["a", "b"] },
    attempts: [],
  };
  const items = inboxItems([task], [task], []);
  assert.equal(items[0].host, "ssh:lab");
});

test("Enter after a digit pick sends it, even right after the selection moved; Enter alone never sends the preselection", async () => {
  const { inboxEnterAnswer } = await load();
  const picked = {
    pick: "Keep behind a flag",
    preselected: "Remove",
    note: "",
  };
  const none = { pick: undefined, preselected: "Remove", note: "" };
  // Selection moved at 1000, "2" at 1150, Enter at 1350: inside the arm window.
  assert.equal(
    inboxEnterAnswer(picked, 1000, 1150, 1350),
    "Keep behind a flag",
  );
  // Digit, then Enter well after the window.
  assert.equal(
    inboxEnterAnswer(picked, 1000, 1150, 1700),
    "Keep behind a flag",
  );
  // Enter alone: the preselection is not an answer, early or late.
  assert.equal(inboxEnterAnswer(none, 1000, undefined, 1200), "");
  assert.equal(inboxEnterAnswer(none, 1000, undefined, 5000), "");
  // A pick made before the selection moved back here does not arm Enter.
  assert.equal(inboxEnterAnswer(picked, 1000, 900, 1200), "");
  assert.equal(inboxEnterAnswer(picked, 1000, 900, 1600), "Keep behind a flag");
});

test("times under a minute read <1m, and the subtitle counts only what the filters show", async () => {
  const { ageLabel, scopedHeadline } = await load();
  assert.equal(ageLabel(0), "<1m");
  assert.equal(ageLabel(59_999), "<1m");
  assert.equal(ageLabel(60_000), "1m");
  const item = (project, host, at) => ({ project, host, at });
  const now = 10 * 60_000;
  assert.equal(
    scopedHeadline([item("alpha", "local", now - 5000)], true, now),
    "1 thing needs you across 1 project on 1 host · oldest <1m",
  );
  assert.equal(
    scopedHeadline(
      [item("alpha", "local", now - 120_000), item("beta", "lab", now)],
      false,
      now,
    ),
    "2 things need you across 2 projects on 2 hosts · oldest 2m",
  );
  assert.equal(scopedHeadline([], true, now), "Nothing here needs you");
});

test("the branch box facts omit a zero cost and an unknown cap", async () => {
  const { diffFacts } = await load();
  assert.equal(diffFacts(3, 1, 4, 0.04), "3 files · attempt 1/4 · $0.04");
  assert.equal(diffFacts(1, 1, undefined, 0), "1 file · attempt 1");
  assert.equal(diffFacts(undefined, 0, 4, undefined), "");
});

test("Inbox zero always says how the day went", async () => {
  const { zeroLine } = await load();
  const now = Date.now();
  assert.equal(
    zeroLine([], now),
    "Nothing needs you. Nothing landed today yet.",
  );
  assert.equal(
    zeroLine([task({ landedSha: "abc", updatedAt: now })], now),
    "Nothing needs you. 1 task landed today.",
  );
});

test("with the orchestrator off the Inbox holds exactly the session items and a plain zero line", async () => {
  const { inboxItems, zeroLine } = await load();
  const sessions = groups([row("b", "blocked"), row("w", "working")]);
  const before = inboxItems([], [], sessions);
  const off = inboxItems(
    [task({ id: "q", status: "waiting" })],
    [task({ id: "done1" })],
    sessions,
    false,
  );
  assert.deepEqual(off, before);
  assert.ok(off.every((item) => item.source === "session"));
  assert.equal(
    zeroLine([task({ id: "done1", landedSha: "abc" })], Date.now(), false),
    "Nothing needs you.",
  );
});

test("a session with an open ask is an ANSWER whatever its status says", async () => {
  const { inboxItems } = await load();
  const items = inboxItems(
    [],
    [],
    groups([row("w", "working"), row("i", "idle")]),
    true,
    new Set(["w"]),
  );
  assert.deepEqual(
    items.map((item) => [item.kind, item.key]),
    [
      ["answer", "panel:w"],
      ["idle", "panel:i"],
    ],
  );
});
