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
