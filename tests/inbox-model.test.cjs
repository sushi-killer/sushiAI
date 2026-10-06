const { test } = require("node:test");
const assert = require("node:assert/strict");

const load = () => import("../src/app/inboxModel.ts");
const entry = (kind, key, over = {}) => ({
  extensionId: "builtin.test",
  item: {
    key,
    kind,
    title: key,
    project: "app",
    host: "local",
    at: 1000,
    search: key,
    ...over,
  },
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

test("module answers mix into the same ANSWER group as sessions", async () => {
  const { inboxItems } = await load();
  const items = inboxItems(
    [entry("answer", "q", { at: 5 }), entry("review", "done1")],
    groups([row("b", "blocked", { since: 9 })]),
  );
  assert.deepEqual(
    items.map((item) => [item.kind, item.source]),
    [
      ["answer", "session"],
      ["answer", "module"],
      ["review", "module"],
    ],
  );
  assert.equal(items[1].key, "builtin.test:q");
});

test("Review all covers exactly the review rows a filter leaves on screen, per module", async () => {
  const { inboxItems, inScope, reviewTargets } = await load();
  const items = inboxItems(
    [
      entry("review", "a", { search: "a app" }),
      entry("review", "b", { project: "site", search: "b site" }),
      entry("review", "c", { search: "c other" }),
      entry("answer", "q"),
      { ...entry("review", "x"), extensionId: "builtin.other" },
    ],
    groups([]),
  );
  const visible = items.filter((item) =>
    inScope(item, { project: "app", host: "", query: "" }),
  );
  assert.deepEqual(reviewTargets(visible), [
    { extensionId: "builtin.test", keys: ["a", "c"] },
    { extensionId: "builtin.other", keys: ["x"] },
  ]);
  const searched = items.filter((item) =>
    inScope(item, { project: "", host: "", query: "OTHER" }),
  );
  assert.deepEqual(reviewTargets(searched), [
    { extensionId: "builtin.test", keys: ["c"] },
  ]);
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

test("a module item keeps the host it reports", async () => {
  const { inboxItems } = await load();
  const items = inboxItems([entry("answer", "t1", { host: "ssh:lab" })], []);
  assert.equal(items[0].host, "ssh:lab");
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

test("with no module items the Inbox holds exactly the session items", async () => {
  const { inboxItems } = await load();
  const sessions = groups([row("b", "blocked"), row("w", "working")]);
  const items = inboxItems([], sessions);
  assert.ok(items.length === 2);
  assert.ok(items.every((item) => item.source === "session"));
});

test("a session with an open ask is an ANSWER whatever its status says", async () => {
  const { inboxItems } = await load();
  const items = inboxItems(
    [],
    groups([row("w", "working"), row("i", "idle")]),
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
