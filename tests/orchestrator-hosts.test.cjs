const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  hostOf,
  routeProblem,
  unavailableRoutes,
  preflightProblems,
  hostsInUse,
  repoSuggestions,
} = require("../src/orchestrator/hosts.ts");
const { orchestratorTarget } = require("../src/orchestrator/notices.ts");
const {
  publishReveal,
  pendingReveal,
  resetReveal,
} = require("../src/orchestrator/reveal.ts");
const {
  ownerTasks,
  ownerTarget,
} = require("../src/orchestrator/ownerAttention.ts");

const task = (id, over = {}) => ({
  id,
  title: `Task ${id}`,
  repo: "/srv/app",
  status: "waiting",
  updatedAt: 1,
  archived: false,
  decisions: [],
  attempts: [],
  question: { text: "Which?" },
  ...over,
});

const preflight = (claude, codex, git = true) => ({
  git,
  claude,
  codex,
  checkedAt: 1,
});

test("a route is unavailable on a host whose harness CLI is missing or logged out", () => {
  const routes = [
    { id: "a", label: "A", harness: "claude" },
    { id: "b", label: "B", harness: "codex" },
  ];
  const state = preflight(
    { installed: true, loggedIn: true },
    { installed: false, loggedIn: false },
  );
  assert.deepEqual(unavailableRoutes(state, routes), {
    b: "codex is not installed on this host",
  });
  const loggedOut = preflight(
    { installed: true, loggedIn: false },
    { installed: true, loggedIn: true },
  );
  assert.equal(
    routeProblem(loggedOut, routes[0]),
    "claude is not logged in on this host",
  );
  // Never probed (Local, or not connected yet): nothing is ruled out.
  assert.deepEqual(unavailableRoutes(null, routes), {});
});

test("preflightProblems names what a host lacks", () => {
  assert.deepEqual(
    preflightProblems(
      preflight(
        { installed: false, loggedIn: false },
        { installed: true, loggedIn: false },
        false,
      ),
    ),
    [
      "git is not installed",
      "claude is not installed",
      "codex is not logged in",
    ],
  );
  assert.deepEqual(preflightProblems(null), []);
});

test("hosts and repo suggestions", () => {
  assert.equal(hostOf({}), "local");
  assert.equal(hostOf({ host: "ssh:x" }), "ssh:x");
  assert.equal(
    hostsInUse([
      { id: "local", enabled: true },
      { id: "ssh:a", enabled: true },
      { id: "ssh:b", enabled: false },
    ]),
    2,
  );
  // Workspaces opened on the host first, then its tasks' repos, each once.
  assert.deepEqual(
    repoSuggestions(
      "ssh:a",
      [
        { cwd: "/home/me/app", connection: "ssh:a" },
        { cwd: "/local/thing" },
        { cwd: "/home/me/other", connection: "ssh:b" },
      ],
      [
        { repo: "/srv/old", updatedAt: 1 },
        { repo: "/home/me/app", updatedAt: 5 },
        { repo: "/srv/new", updatedAt: 9 },
      ],
    ),
    ["/home/me/app", "/srv/new", "/srv/old"],
  );
});

test("badge and Inbox rows include tasks from every host and opening one selects its host", () => {
  const tasks = [
    task("local-1", { updatedAt: 2 }),
    task("remote-1", { host: "ssh:a", updatedAt: 3 }),
    task("remote-2", { host: "ssh:a", status: "running", updatedAt: 4 }),
    task("remote-3", { host: "ssh:b", status: "failed", updatedAt: 1 }),
  ];
  // The badge counts what needs the owner, on every host.
  assert.deepEqual(
    ownerTasks(tasks).map((t) => t.id),
    ["remote-1", "local-1", "remote-3"],
  );
  assert.deepEqual(ownerTarget(tasks[1]), {
    taskId: "remote-1",
    repo: "/srv/app",
    host: "ssh:a",
    focus: "question",
  });
  assert.equal("host" in ownerTarget(tasks[0]), false);
});

test("opening a remote task picks a panel on its host, else any panel, else adds one", () => {
  const leaf = (id) => ({ type: "leaf", id });
  const panel = (id) => ({ id, kind: "orchestrator", title: "O" });
  const ws = (id, cwd, connection, panels) => ({
    id,
    name: id,
    cwd,
    connection,
    panels,
    layout: panels.length ? leaf(panels[0].id) : null,
  });
  const local = ws("w-local", "/srv/app", undefined, [panel("p-local")]);
  const remote = ws("w-remote", "/home/me/app", "ssh:a", [panel("p-remote")]);
  const bare = ws("w-bare", "/x", "ssh:a", []);
  assert.deepEqual(orchestratorTarget([local, remote], "/srv/app", "ssh:a"), {
    kind: "panel",
    workspaceId: "w-remote",
    panelId: "p-remote",
  });
  assert.deepEqual(orchestratorTarget([local, bare], "/srv/app", "ssh:a"), {
    kind: "panel",
    workspaceId: "w-local",
    panelId: "p-local",
  });
  assert.deepEqual(
    orchestratorTarget([ws("w", "/y", undefined, [])], "/srv/app", "ssh:a"),
    { kind: "add-panel", workspaceId: "w" },
  );
  // A local task still finds its workspace by cwd.
  assert.equal(
    orchestratorTarget([local, remote], "/srv/app").workspaceId,
    "w-local",
  );
});

test("a remote reveal is taken by any panel; a local one only by its repo's panel", () => {
  resetReveal();
  publishReveal({
    taskId: "t",
    repo: "/remote/path",
    host: "ssh:a",
    focus: "summary",
  });
  assert.equal(pendingReveal("/anything").taskId, "t");
  resetReveal();
  publishReveal({ taskId: "t", repo: "/srv/app", focus: "summary" });
  assert.equal(pendingReveal("/other"), null);
  assert.equal(pendingReveal("/srv/app").taskId, "t");
  resetReveal();
});
