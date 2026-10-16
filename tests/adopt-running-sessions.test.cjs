const { test } = require("node:test");
const assert = require("node:assert/strict");

const daemon = import("../src/daemonSessions.ts");
const attention = import("../src/app/attention.ts");
const launch = import("../src/workspace/session-launch.ts");
const layout = import("../src/layout.ts");
const state = import("../src/workspaceState.ts");

const session = (id, extra = {}) => ({
  id,
  cmd: ["claude"],
  cwd: "/work/app",
  status: "running",
  cols: 80,
  rows: 24,
  ...extra,
});
const ready = (sessions) => ({
  ready: true,
  listed: true,
  sessions: Object.fromEntries(sessions.map((s) => [s.id, s])),
});
const own = {
  id: "p1",
  kind: "agent",
  title: "Mine",
  agent: "claude",
  sessionId: "s1",
  paneCwd: "/work/app",
};
const workspace = (id, panels, extra = {}) => ({
  id,
  name: id,
  cwd: "/work/app",
  connection: "local",
  panels,
  layout: panels.length ? { type: "leaf", id: panels[0].id } : null,
  ...extra,
});

function bound(workspaces, sessionId) {
  const out = [];
  for (const w of workspaces) {
    for (const p of w.panels) {
      if (p.sessionId === sessionId) out.push({ w, p });
    }
  }
  return out;
}

test("a running session no panel shows is adopted into the workspace of its host and folder", async () => {
  const { reconcileSessions, sessionPanelId } = await daemon;
  const { codePanels } = await state;
  const before = [workspace("w1", [own])];
  const after = reconcileSessions(before, {
    local: ready([
      session("s1"),
      session("s2", {
        agent: "claude",
        agentStatus: "working",
        title: "Fix the login",
        cwd: "/work/app/web",
      }),
    ]),
  });
  assert.equal(after.length, 1);
  const found = bound(after, "s2");
  assert.equal(found.length, 1);
  const { w, p } = found[0];
  assert.equal(w.id, "w1");
  assert.equal(p.id, sessionPanelId("local", "s2"));
  assert.equal(p.kind, "agent");
  assert.equal(p.agent, "claude");
  assert.equal(p.title, "Fix the login");
  assert.equal(p.status, "working");
  assert.equal(p.paneCwd, "/work/app/web");
  assert.equal(p.ended, undefined);
  assert.deepEqual(w.layout, before[0].layout, "the canvas is untouched");
  assert.ok(
    codePanels(w).some((x) => x.id === p.id),
    "the sidebar lists it",
  );
  assert.equal(before[0].panels.length, 1, "the input is untouched");
});

test("a shell session is adopted as a terminal with the agent started in it", async () => {
  const { reconcileSessions } = await daemon;
  const after = reconcileSessions([workspace("w1", [own])], {
    local: ready([
      session("s1"),
      session("s3", { cmd: ["zsh"], foregroundAgent: "codex" }),
    ]),
  });
  const [{ p }] = bound(after, "s3");
  assert.equal(p.kind, "terminal");
  assert.equal(p.agent, "codex");
});

test("exited and background sessions are never adopted", async () => {
  const { reconcileSessions } = await daemon;
  const before = [workspace("w1", [own])];
  const hosts = {
    local: ready([
      session("s1"),
      session("gone", { status: "exited", agentStatus: "exited" }),
      session("job", { agent: "claude", background: true }),
    ]),
  };
  assert.equal(reconcileSessions(before, hosts), before);
});

test("nothing is adopted from a host that is not ready or not listed", async () => {
  const { reconcileSessions } = await daemon;
  const before = [workspace("w1", [own])];
  const live = ready([session("s1"), session("s2", { agent: "claude" })]);
  const notReady = { ...live, ready: false };
  const notListed = { ...live, listed: false };
  for (const host of [notReady, notListed]) {
    assert.equal(reconcileSessions(before, { local: host }), before);
  }
});

test("a session in a folder no workspace holds gets a workspace on its own host", async () => {
  const { reconcileSessions, sessionPanelId } = await daemon;
  const before = [workspace("w1", [own])];
  const hosts = {
    local: ready([session("s1")]),
    devbox: ready([
      session("r1", { agent: "codex", cwd: "/srv/api" }),
      session("r2", { cmd: ["bash"], cwd: "/srv/api" }),
      session("r3", { agent: "claude", cwd: "/work/app" }),
    ]),
  };
  const after = reconcileSessions(before, hosts);
  const local = after.find((w) => w.id === "w1");
  assert.deepEqual(
    local.panels.map((p) => p.sessionId),
    ["s1"],
  );
  const remote = after.filter((w) => w.connection === "ssh:devbox");
  assert.equal(remote.length, 2, "one workspace per folder");
  const api = remote.find((w) => w.cwd === "/srv/api");
  assert.equal(api.name, "api");
  const ids = api.panels.map((p) => p.sessionId).sort();
  assert.deepEqual(ids, ["r1", "r2"]);
  const r1 = api.panels.find((p) => p.sessionId === "r1");
  assert.equal(r1.id, sessionPanelId("ssh:devbox", "r1"));
  assert.equal(reconcileSessions(after, hosts), after, "stable");
});

test("a session goes to the workspace it was launched for on its host", async () => {
  const { reconcileSessions } = await daemon;
  const other = workspace("w2", [], { cwd: "/work/other" });
  const before = [workspace("w1", [own]), other];
  const after = reconcileSessions(before, {
    local: ready([
      session("s1"),
      session("s5", { agent: "claude", group: "w2" }),
    ]),
  });
  const found = bound(after, "s5");
  assert.equal(found.length, 1);
  assert.equal(found[0].w.id, "w2");
});

test("an adopted session is listed in the Inbox with its status", async () => {
  const { reconcileSessions } = await daemon;
  const { inboxGroups, waitingCount, createAttentionState } = await attention;
  const after = reconcileSessions([workspace("w1", [own])], {
    local: ready([
      session("s1"),
      session("s6", { agent: "claude", agentStatus: "blocked" }),
    ]),
  });
  const seen = createAttentionState();
  const groups = inboxGroups(after, seen, []);
  const blocked = groups.find((group) => group.key === "blocked");
  assert.deepEqual(
    blocked.rows.map((row) => row.panel.sessionId),
    ["s6"],
  );
  assert.equal(waitingCount(after, seen, []), 1);
});

test("a launch whose session event came first still opens one panel", async () => {
  const { reconcileSessions, sessionPanelId } = await daemon;
  const { placeLaunchedPanel } = await launch;
  const { contains } = await layout;
  const hosts = {
    local: ready([
      session("s1"),
      session("s9", { agent: "claude", group: "w1" }),
    ]),
  };
  const adopted = reconcileSessions([workspace("w1", [own])], hosts);
  const panel = {
    id: sessionPanelId("local", "s9"),
    kind: "agent",
    title: "Claude",
    agent: "claude",
    sessionId: "s9",
    started: true,
  };
  const request = {
    operationId: "op",
    workspaceId: "w1",
    endpoint: "local",
    kind: "agent",
    agent: "claude",
    cwd: "/work/app",
    label: "app",
  };
  const placed = placeLaunchedPanel(adopted, {
    targetId: "w1",
    request,
    panel,
    cwd: "/work/app",
  });
  const found = bound(placed, "s9");
  assert.equal(found.length, 1);
  assert.equal(found[0].w.id, "w1");
  assert.ok(contains(found[0].w.layout, panel.id), "it is on the canvas");
});

test("a launch into a new workspace still opens one panel in one workspace", async () => {
  const { reconcileSessions, sessionPanelId } = await daemon;
  const { placeLaunchedPanel, launchTarget } = await launch;
  const { contains } = await layout;
  const hosts = {
    local: ready([
      session("s1"),
      session("s8", { agent: "codex", group: "ws-op2", cwd: "/work/new" }),
    ]),
  };
  const adopted = reconcileSessions([workspace("w1", [own])], hosts);
  const panel = {
    id: sessionPanelId("local", "s8"),
    kind: "agent",
    title: "Codex",
    agent: "codex",
    sessionId: "s8",
    started: true,
  };
  const request = {
    operationId: "op2",
    workspaceId: "ws-op2",
    endpoint: "local",
    kind: "agent",
    agent: "codex",
    cwd: "/work/new",
    label: "new",
  };
  const targetId = launchTarget(adopted, request, panel.id) || "ws-op2";
  const placed = placeLaunchedPanel(adopted, {
    targetId,
    request,
    panel,
    cwd: "/work/new",
  });
  const found = bound(placed, "s8");
  assert.equal(found.length, 1);
  assert.ok(contains(found[0].w.layout, panel.id), "it is on the canvas");
  assert.equal(placed.filter((w) => w.cwd === "/work/new").length, 1);
});
