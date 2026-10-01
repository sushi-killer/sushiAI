// Project values reach sessions through the entry points the renderer really
// uses: a terminal opened with only a folder (it sends no project id), and a
// Herdr pane that is typed a prefix. The fake host shows what it received.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { registerTerminalIpc } = require("../electron/ipc/terminals.cjs");
const { makeHost, makeStore } = require("./helpers/fake-host.cjs");

const SECRET = "invented-session-secret";
const ACCOUNTS = {
  sub: { kind: "subscription", value: "invented-oauth-token" },
  key: { kind: "apiKey", value: "invented-api-key" },
};

async function rig(
  t,
  { attach = true, withheld = false, sessions = undefined } = {},
) {
  const host = await makeHost(t, {
    bin: { mktemp: '#!/bin/sh\nexec /usr/bin/mktemp "$HOME/tmp.XXXXXX"\n' },
  });
  const { projects } = await makeStore(t);
  const project = await projects.upsert({
    name: "App",
    git: { url: "git@example.test:acme/app.git", defaultBranch: "main" },
    env: [
      { name: "APP_TOKEN", secret: true },
      { name: "PLAIN", secret: false },
    ],
    sessions,
  });
  await projects.setSecret(project.id, "APP_TOKEN", SECRET);
  await projects.setSecret(project.id, "PLAIN", "visible");
  const cwd = path.join(host.home, "code", "app");
  await fs.mkdir(cwd, { recursive: true });
  if (attach)
    await projects.attach({
      remote: project.git.url,
      endpoint: host.endpoint,
      cwd,
      name: "App",
    });
  if (withheld) await projects.setHostWithheld(project.id, host.endpoint, true);
  const spawned = [];
  const handlers = new Map();
  registerTerminalIpc({
    handle: (channel, callback) => handlers.set(channel, callback),
    send: () => {},
    app: {},
    pty: {
      spawn: (binary, args) => {
        spawned.push([binary, ...args]);
        return { onData: () => {}, onExit: () => {} };
      },
    },
    getConnections: () => host.connections,
    executable: (name) => `/usr/bin/${name}`,
    directory: () => {},
    id: () => {},
    terminals: new Map(),
    terminalPending: new Map(),
    stageModelSettings: async () => "",
    stageClaudeAccount: async () => ({}),
    resolveClaudeAccount: async (id) => ACCOUNTS[id],
    resolveModel: async () => ({
      key: "invented-model-key",
      settings: { ANTHROPIC_BASE_URL: "https://models.example.test" },
    }),
    projects,
    sshBinary: host.ssh,
  });
  return { host, projects, project, cwd, handlers, spawned };
}

const left = async (host) =>
  (await fs.readdir(host.home)).filter((name) => name.startsWith("tmp."));

test("a terminal opened with only a folder still gets the project's values on the host", async (t) => {
  const { host, cwd, handlers, spawned } = await rig(t);
  // Exactly what the renderer sends: no projectId.
  await handlers.get("terminal-open")({
    panelId: "p1",
    cwd,
    command: "claude",
    endpoint: host.endpoint,
  });
  // The values went up as a file and the ssh command only sources it.
  const files = await left(host);
  assert.equal(files.length, 1);
  const text = await fs.readFile(path.join(host.home, files[0]), "utf8");
  assert.match(text, new RegExp(`APP_TOKEN='${SECRET}'`));
  assert.equal(JSON.stringify(spawned).includes(SECRET), false);
});

test("a folder the project is known by through its remote gets values too", async (t) => {
  const { host, projects, project, cwd, handlers } = await rig(t, {
    attach: false,
  });
  // The folder is a real checkout of the project's repository.
  const { execFileSync } = require("node:child_process");
  execFileSync("git", ["init", "-q", "-b", "main", cwd]);
  execFileSync("git", ["-C", cwd, "remote", "add", "origin", project.git.url]);
  await handlers.get("terminal-open")({
    panelId: "p2",
    cwd,
    command: "claude",
    endpoint: host.endpoint,
  });
  assert.equal((await left(host)).length, 1);
  assert.ok(projects);
});

test("a host switched off for the project gets no file from a terminal", async (t) => {
  const { host, cwd, handlers } = await rig(t, { withheld: true });
  await handlers.get("terminal-open")({
    panelId: "p3",
    cwd,
    command: "claude",
    endpoint: host.endpoint,
  });
  assert.deepEqual(await left(host), []);
});

test("a Herdr pane is typed a prefix that sources the values once and leaves no file", async (t) => {
  const { host, cwd, handlers } = await rig(t);
  const { prefix } = await handlers.get("project-session-env")({
    endpoint: host.endpoint,
    cwd,
  });
  // The text typed into the pane holds no value.
  assert.equal(prefix.includes(SECRET), false);
  assert.equal((await left(host)).length, 1);
  // What the pane's shell does with it: the values are in its environment ...
  await host.connections.exec(
    host.endpoint,
    `${prefix}printf '%s|%s' "$APP_TOKEN" "$PLAIN" > "$HOME/seen"`,
  );
  assert.equal(
    await fs.readFile(path.join(host.home, "seen"), "utf8"),
    `${SECRET}|visible`,
  );
  // ... and the file is gone.
  assert.deepEqual(await left(host), []);
});

test("a Herdr pane on a host switched off, or in a folder with no project, is typed nothing", async (t) => {
  const off = await rig(t, { withheld: true });
  assert.deepEqual(
    await off.handlers.get("project-session-env")({
      endpoint: off.host.endpoint,
      cwd: off.cwd,
    }),
    { prefix: "" },
  );
  const none = await rig(t, { attach: false });
  assert.deepEqual(
    await none.handlers.get("project-session-env")({
      endpoint: none.host.endpoint,
      cwd: none.cwd,
    }),
    { prefix: "" },
  );
});

const seen = async (host, prefix, names) => {
  await host.connections.exec(
    host.endpoint,
    `${prefix}printf '%s|' ${names.map((n) => `"$${n}"`).join(" ")} > "$HOME/seen"`,
  );
  return fs.readFile(path.join(host.home, "seen"), "utf8");
};

test("a Herdr pane gets the picked Claude account even in a folder with no project", async (t) => {
  const { host, cwd, handlers } = await rig(t, { attach: false });
  const { prefix } = await handlers.get("project-session-env")({
    endpoint: host.endpoint,
    cwd,
    claudeAccountId: "sub",
  });
  assert.equal(prefix.includes("invented-oauth-token"), false);
  assert.equal(
    await seen(host, prefix, ["CLAUDE_CODE_OAUTH_TOKEN"]),
    "invented-oauth-token|",
  );
  assert.deepEqual(await left(host), []);
});

test("a Herdr pane runs as the project's own account when none is picked, an API key as one", async (t) => {
  const { host, cwd, handlers } = await rig(t, {
    sessions: { claudeAccount: "key" },
  });
  const { prefix } = await handlers.get("project-session-env")({
    endpoint: host.endpoint,
    cwd,
  });
  assert.equal(
    await seen(host, prefix, ["APP_TOKEN", "ANTHROPIC_API_KEY"]),
    `${SECRET}|invented-api-key|`,
  );
});

test("a host switched off for the project gets no Claude account either", async (t) => {
  const { host, cwd, handlers } = await rig(t, { withheld: true });
  assert.deepEqual(
    await handlers.get("project-session-env")({
      endpoint: host.endpoint,
      cwd,
      claudeAccountId: "sub",
    }),
    { prefix: "" },
  );
});

test("a host with no Codex login gets the local one, a host with one keeps it", async (t) => {
  const { seedCodexLogin } = require("../electron/project-session.cjs");
  const { host } = await rig(t, { attach: false });
  const local = path.join(host.root, "local-codex");
  await fs.mkdir(local);
  await fs.writeFile(path.join(local, "auth.json"), '{"tokens":"mine"}');
  await seedCodexLogin(host.connections, host.endpoint, local);
  const remote = path.join(host.home, ".codex", "auth.json");
  assert.equal(await fs.readFile(remote, "utf8"), '{"tokens":"mine"}');
  assert.equal((await fs.stat(remote)).mode & 0o777, 0o600);
  await fs.writeFile(path.join(local, "auth.json"), '{"tokens":"newer"}');
  await seedCodexLogin(host.connections, host.endpoint, local);
  assert.equal(await fs.readFile(remote, "utf8"), '{"tokens":"mine"}');
});

test("a remote pane runs a custom model: the key in its shell, the settings inline, neither key in the text", async (t) => {
  const { remoteModelLaunch } = require("../electron/project-session.cjs");
  const { quote } = require("../electron/connections.cjs");
  const { host, cwd, handlers } = await rig(t, { attach: false });
  const { prefix } = await handlers.get("project-session-env")({
    endpoint: host.endpoint,
    cwd,
    agent: "claude",
    modelProfileId: "m1",
  });
  const launch = await handlers.get("model-launch-remote")("m1");
  assert.equal((prefix + launch).includes("invented-model-key"), false);
  const document = JSON.parse(
    launch.slice("claude --settings '".length, -1).replaceAll("'\\''", "'"),
  );
  assert.equal(document.env.ANTHROPIC_BASE_URL, "https://models.example.test");
  assert.equal(launch, remoteModelLaunch(document.env));
  // What Claude Code does with apiKeyHelper in the pane's shell.
  await host.connections.exec(
    host.endpoint,
    `${prefix}sh -c ${quote(document.apiKeyHelper)} > "$HOME/seen"`,
  );
  assert.equal(
    await fs.readFile(path.join(host.home, "seen"), "utf8"),
    "invented-model-key",
  );
});
