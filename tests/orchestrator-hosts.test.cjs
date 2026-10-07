const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  hostOf,
  routeProblem,
  unavailableRoutes,
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

test("hosts and repo suggestions", () => {
  assert.equal(hostOf({}), "local");
  assert.equal(hostOf({ host: "ssh:x" }), "ssh:x");
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
  const panel = (id) => ({
    id,
    kind: "extension",
    title: "O",
    extension: {
      extensionId: "builtin.orchestrator",
      contributionId: "orchestration",
      instanceId: id,
      stateVersion: 1,
    },
  });
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
  // ...and never lands on an SSH workspace that has the same path.
  assert.equal(
    orchestratorTarget([remote], "/home/me/app").kind,
    "create-workspace",
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

const UPDATE_ERROR = "Update sushiai on lab";
const NOT_INSTALLED_ERROR =
  "sushiai is not installed on this host. Install it first.";

test("the host selector's second line and dot follow the host and its daemon", () => {
  const { hostStatus } = require("../src/orchestrator/hosts.ts");
  const ssh = (state, detail = "") => ({
    id: "ssh:a",
    name: "lab",
    state,
    detail,
    enabled: true,
  });
  const local = { id: "local", name: "Local", state: "ready", enabled: true };
  assert.deepEqual(hostStatus(local, "ready"), {
    tone: "ok",
    detail: "This Mac · connected",
  });
  assert.deepEqual(hostStatus(local, "unavailable"), {
    tone: "danger",
    detail: "This Mac · sushiai unreachable",
  });
  assert.deepEqual(hostStatus(ssh("ready")), {
    tone: "ok",
    detail: "SSH · sushiai connected",
  });
  assert.equal(
    hostStatus(ssh("connecting")).detail,
    "SSH · connecting to sushiai",
  );
  assert.deepEqual(hostStatus(ssh("error", UPDATE_ERROR)), {
    tone: "warning",
    detail: "SSH · sushiai needs an update",
  });
  assert.equal(
    hostStatus(ssh("error", "The SSH connection dropped.")).tone,
    "danger",
  );
  // A ready host whose daemon stopped answering the panel is unreachable.
  assert.equal(
    hostStatus(ssh("ready"), "unavailable").detail,
    "SSH · sushiai unreachable",
  );
  assert.equal(
    hostStatus({ ...ssh("idle"), enabled: false }).detail,
    "SSH · not set up",
  );
});

test("a remote host shows the setup card until it answers once; then its tasks stay", () => {
  const { needsSetup } = require("../src/orchestrator/hosts.ts");
  const host = (state) => ({ id: "ssh:a", state });
  assert.equal(needsSetup({ id: "local", state: "error" }, false), false);
  assert.equal(needsSetup(host("connecting"), false), true);
  assert.equal(needsSetup(host("connecting"), true), false);
  assert.equal(needsSetup(host("error"), false), true);
  assert.equal(needsSetup(host("error"), true), false);
  assert.equal(needsSetup(host("ready"), false), false);
});

test("the setup steps follow the host's state and offer the install where it helps", () => {
  const { setupSteps } = require("../src/orchestrator/hosts.ts");
  const seen = (states) => ({ states, address: "user@devbox" });
  const states = (steps) => steps.map((step) => step.state);

  const connecting = setupSteps({ state: "connecting" }, seen([]));
  assert.deepEqual(states(connecting), ["active", "pending", "pending"]);
  assert.equal(connecting[0].detail, "user@devbox");

  const reaching = setupSteps({ state: "connecting" }, seen(["connecting"]));
  assert.deepEqual(states(reaching), ["done", "active", "pending"]);
  assert.equal(reaching[0].title, "Connected over SSH");

  const dropped = setupSteps(
    { state: "error", detail: "The SSH connection dropped." },
    seen([]),
  );
  assert.deepEqual(states(dropped), ["failed", "pending", "pending"]);
  assert.equal(dropped[0].detail, "The SSH connection dropped.");
  assert.equal(
    dropped.some((step) => step.action),
    false,
  );

  const missing = setupSteps(
    { state: "error", detail: NOT_INSTALLED_ERROR, name: "Devbox" },
    seen(["connecting"]),
  );
  assert.deepEqual(states(missing), ["done", "failed", "pending"]);
  assert.equal(missing[1].action.label, "Install sushiai");
  assert.match(missing[1].action.hint, /on Devbox \(no sudo\)/);

  // The `orch` capability is what the last step checks.
  const old = setupSteps(
    { state: "error", detail: UPDATE_ERROR, name: "Devbox" },
    seen(["connecting"]),
  );
  assert.deepEqual(states(old), ["done", "done", "failed"]);
  assert.equal(old[2].detail, UPDATE_ERROR);
  assert.equal(old[2].action.label, "Update sushiai");

  const ready = setupSteps({ state: "ready" }, seen(["connecting"]));
  assert.deepEqual(states(ready), ["done", "done", "done"]);
});

test("the preflight strip lists git and each CLI, and names the routes it turns off", () => {
  const {
    preflightItems,
    routesFallbackNote,
  } = require("../src/orchestrator/hosts.ts");
  const state = preflight(
    { installed: true, loggedIn: true },
    { installed: false, loggedIn: false },
  );
  assert.deepEqual(preflightItems(state), [
    { name: "git", ok: true, note: "" },
    { name: "claude", ok: true, note: "logged in" },
    { name: "codex", ok: false, note: "not installed" },
  ]);
  assert.equal(
    routesFallbackNote(state, "lab"),
    "codex is not installed on lab; its routes fall back to an installed CLI.",
  );
  assert.equal(
    routesFallbackNote(
      preflight(
        { installed: true, loggedIn: false },
        { installed: true, loggedIn: true },
      ),
      "lab",
    ),
    "claude is not logged in on lab; its routes fall back to an installed CLI.",
  );
  assert.equal(
    routesFallbackNote(
      preflight(
        { installed: true, loggedIn: true },
        { installed: true, loggedIn: true },
      ),
      "lab",
    ),
    "",
  );
});

test("a host's running count skips the drafts that wait on the Plan", () => {
  const { runningCount } = require("../src/orchestrator/hosts.ts");
  const attempt = { n: 1, stage: "implement" };
  const tasks = [
    task("run", { status: "running", attempts: [attempt] }),
    // Queued, never started, no lease: a Plan draft, not running.
    task("draft", { status: "queued" }),
    // Queued behind another task's lease after starting: running.
    task("lease", {
      status: "queued",
      queueReason: 'waits for "A" on src/x',
      attempts: [attempt],
    }),
    task("backlog", { status: "queued", backlog: { bucket: "next" } }),
    task("done", { status: "done", attempts: [attempt] }),
  ];
  assert.equal(runningCount(tasks), 2);
});

test("a host the main process retries after a failure keeps showing that failure", () => {
  const { shownHost } = require("../src/orchestrator/hosts.ts");
  const retrying = { id: "ssh:a", state: "connecting", detail: "" };
  assert.deepEqual(shownHost(retrying, UPDATE_ERROR), {
    ...retrying,
    state: "error",
    detail: UPDATE_ERROR,
  });
  assert.equal(shownHost(retrying, "").state, "connecting");
  // Past connecting, the retry is real progress: it shows.
  assert.equal(
    shownHost({ ...retrying, state: "ready" }, UPDATE_ERROR).state,
    "ready",
  );
});
