const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  RUSTUP_COMMAND,
  needsRustMessage,
  noSourceMessage,
} = require("../electron/orchestrator-remote.cjs");
const {
  hostOf,
  hostPlatform,
  rustupCommand,
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

const RUST_ERROR =
  "Rust is not installed on devbox (Linux aarch64), and orchd has to be built there. Install it on the host with: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y - then connect again.";

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
    detail: "This Mac · orchd unreachable",
  });
  assert.deepEqual(hostStatus(ssh("ready")), {
    tone: "ok",
    detail: "SSH · orchd connected",
  });
  for (const state of ["connecting", "installing", "building", "starting"])
    assert.equal(hostStatus(ssh(state)).detail, "SSH · setting up orchd");
  assert.deepEqual(hostStatus(ssh("error", RUST_ERROR)), {
    tone: "warning",
    detail: "SSH · orchd needs Rust",
  });
  assert.equal(
    hostStatus(ssh("error", "The SSH connection dropped.")).tone,
    "danger",
  );
  // A ready host whose daemon stopped answering the panel is unreachable.
  assert.equal(
    hostStatus(ssh("ready"), "unavailable").detail,
    "SSH · orchd unreachable",
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
  assert.equal(needsSetup(host("building"), false), true);
  assert.equal(needsSetup(host("connecting"), true), false);
  assert.equal(needsSetup(host("error"), false), true);
  assert.equal(needsSetup(host("error"), true), false);
  assert.equal(needsSetup(host("ready"), false), false);
});

test("the setup steps follow the host's state, with the elapsed build time", () => {
  const { setupSteps } = require("../src/orchestrator/hosts.ts");
  const seen = (states, elapsedMs = 80_000) => ({
    states,
    elapsedMs,
    address: "user@devbox",
  });
  const building = setupSteps(
    { state: "building", detail: "Building orchd on devbox (a few minutes)" },
    seen(["connecting", "building"]),
  );
  assert.deepEqual(
    building.map((step) => step.state),
    ["done", "done", "active", "pending", "pending"],
  );
  assert.equal(building[0].title, "Connected over SSH");
  assert.equal(building[0].detail, "user@devbox");
  assert.equal(building[1].detail, "no current build at ~/.sushiai/bin/orchd");
  assert.equal(building[2].title, "Building orchd from source");
  assert.match(building[2].detail, /cargo build --release · 1m 20s$/);

  // The platform the SSH probe read, before any error names it.
  const probed = setupSteps(
    {
      state: "building",
      detail: "Building orchd on devbox (a few minutes)",
      platform: "Linux x86_64",
      orchdInstalled: false,
    },
    seen(["connecting", "building"]),
  );
  assert.equal(probed[0].detail, "Linux x86_64 · user@devbox");
  assert.equal(probed[1].detail, "not installed at ~/.sushiai/bin/orchd");
  assert.match(probed[2].detail, /^no matching build for Linux x86_64 · /);

  const connecting = setupSteps({ state: "connecting" }, seen(["connecting"]));
  assert.deepEqual(
    connecting.map((step) => step.state),
    ["active", "pending", "pending", "pending", "pending"],
  );

  const rust = setupSteps(
    { state: "error", detail: RUST_ERROR },
    seen(["connecting", "error"]),
  );
  assert.deepEqual(
    rust.map((step) => step.state),
    ["done", "done", "failed", "pending", "pending"],
  );
  assert.equal(rust[0].detail, "Linux aarch64 · user@devbox");
  assert.equal(rust[2].title, "Can’t build orchd here");
  assert.equal(
    rust[2].detail,
    "no matching build for Linux aarch64, and cargo isn’t installed",
  );
  assert.equal(
    rust[2].command,
    "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y",
  );

  const silent = setupSteps(
    {
      state: "error",
      detail: "The orchestrator on devbox did not answer. Check the log.",
    },
    seen(["connecting", "starting", "error"]),
  );
  assert.equal(silent[4].state, "failed");
  assert.match(silent[4].detail, /did not answer/);
  assert.equal(silent[2].detail, "already up to date");
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
  assert.deepEqual(shownHost(retrying, RUST_ERROR), {
    ...retrying,
    state: "error",
    detail: RUST_ERROR,
  });
  assert.equal(shownHost(retrying, "").state, "connecting");
  // Past connecting, the retry is real progress: it shows.
  assert.equal(
    shownHost({ ...retrying, state: "building" }, RUST_ERROR).state,
    "building",
  );
});

test("the setup errors main builds parse back into the platform and the rustup command", () => {
  const rust = needsRustMessage("Box", "Linux aarch64");
  assert.equal(hostPlatform(rust), "Linux aarch64");
  assert.equal(rustupCommand(rust), RUSTUP_COMMAND);
  const source = noSourceMessage("Box", "Linux x86_64", "Darwin arm64");
  assert.equal(hostPlatform(source), "Linux x86_64");
  assert.equal(rustupCommand(source), null);
  // An unknown platform names none.
  assert.equal(hostPlatform(needsRustMessage("Box", "")), null);
  assert.equal(rustupCommand(needsRustMessage("Box", "")), RUSTUP_COMMAND);
});

test("a host that needs Rust gets one button, its progress, and a Retry after a failure", () => {
  const {
    setupSteps,
    buildToolsHint,
  } = require("../src/orchestrator/hosts.ts");
  const { rustFailedMessage } = require("../electron/orchestrator-remote.cjs");
  const seen = (states) => ({ states, elapsedMs: 5000 });

  const needs = setupSteps(
    { state: "error", detail: RUST_ERROR, name: "Devbox" },
    seen(["connecting", "error"]),
  );
  assert.equal(needs[2].action.label, "Install Rust and set up");
  assert.equal(
    needs[2].action.hint,
    "Installs a minimal Rust toolchain in ~/.cargo on Devbox (no sudo), then builds and starts the orchestrator.",
  );
  assert.equal(needs[2].command, RUSTUP_COMMAND);

  const installing = setupSteps(
    { state: "building", detail: "Installing Rust on Devbox" },
    seen(["connecting", "error", "building"]),
  );
  assert.equal(installing[2].title, "Installing Rust…");
  assert.equal(installing[2].state, "active");
  assert.equal(installing[2].action, undefined);

  const failed = setupSteps(
    {
      state: "error",
      detail: rustFailedMessage("Devbox", "curl: (6) Could not resolve host"),
      name: "Devbox",
    },
    seen(["connecting", "building", "error"]),
  );
  assert.equal(failed[2].state, "failed");
  assert.equal(failed[2].title, "Couldn’t install Rust");
  assert.equal(failed[2].detail, "curl: (6) Could not resolve host");
  assert.equal(failed[2].action.label, "Retry");

  // An upload host never sees it.
  const upload = setupSteps(
    { state: "installing", detail: "Uploading orchd to Devbox" },
    seen(["connecting", "installing"]),
  );
  assert.equal(
    upload.some((step) => step.action),
    false,
  );

  assert.match(buildToolsHint("Darwin arm64"), /xcode-select --install/);
  assert.match(buildToolsHint("Linux x86_64"), /build-essential/);
});
