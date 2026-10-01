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
const CODEX_ACCOUNTS = {
  work: { home: "/invented/codex-home", auth: '{"tokens":"work"}' },
};

async function rig(
  t,
  { attach = true, withheld = false, sessions = undefined, codex = "" } = {},
) {
  const host = await makeHost(t, {
    bin: {
      mktemp:
        '#!/bin/sh\n[ "$1" = -d ] && exec /usr/bin/mktemp -d "$HOME/tmpd.XXXXXX"\nexec /usr/bin/mktemp "$HOME/tmp.XXXXXX"\n',
      ln: '#!/bin/sh\nexec /bin/ln "$@"\n',
      // Shows what a Codex session sees: its home, its login, its files.
      codex:
        '#!/bin/sh\nprintf \'%s|%s|%s\' "${CODEX_HOME:-}" "$(cat "${CODEX_HOME:-$HOME/.codex}/auth.json" 2>/dev/null)" "$(ls -A "${CODEX_HOME:-$HOME/.codex}" | tr "\\n" ,)" > "$HOME/codex-saw"\n',
    },
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
  const envs = [];
  const handlers = new Map();
  registerTerminalIpc({
    handle: (channel, callback) => handlers.set(channel, callback),
    send: () => {},
    app: {},
    pty: {
      spawn: (binary, args, options) => {
        spawned.push([binary, ...args]);
        envs.push(options.env);
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
    resolveClaudeAccount: async (id) => {
      if (!ACCOUNTS[id]) throw new Error("Add a value for it first.");
      return ACCOUNTS[id];
    },
    codexAuth: async () => codex,
    resolveCodexAccount: async (id) => {
      if (!CODEX_ACCOUNTS[id]) throw new Error("Sign in to it first.");
      return CODEX_ACCOUNTS[id];
    },
    resolveModel: async () => ({
      key: "invented-model-key",
      settings: { ANTHROPIC_BASE_URL: "https://models.example.test" },
    }),
    projects,
    sshBinary: host.ssh,
  });
  return { host, projects, project, cwd, handlers, spawned, envs };
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
    { prefix: "", settings: "", launch: "" },
  );
  const none = await rig(t, { attach: false });
  assert.deepEqual(
    await none.handlers.get("project-session-env")({
      endpoint: none.host.endpoint,
      cwd: none.cwd,
    }),
    { prefix: "", settings: "", launch: "" },
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
    agent: "claude",
    claudeAccountId: "sub",
  });
  assert.equal(prefix.includes("invented-oauth-token"), false);
  assert.equal(
    await seen(host, prefix, ["CLAUDE_CODE_OAUTH_TOKEN"]),
    "invented-oauth-token|",
  );
  assert.deepEqual(await left(host), []);
});

test("Claude runs as the project's own account when none is picked; an API key goes through apiKeyHelper", async (t) => {
  const { quote } = require("../electron/connections.cjs");
  const { host, cwd, handlers } = await rig(t, {
    sessions: { claudeAccount: "key" },
  });
  const { prefix, settings } = await handlers.get("project-session-env")({
    endpoint: host.endpoint,
    cwd,
    agent: "claude",
  });
  assert.equal((prefix + settings).includes("invented-api-key"), false);
  const document = JSON.parse(
    settings.slice(" --settings '".length, -1).replaceAll("'\\''", "'"),
  );
  // Interactive Claude Code drops an unapproved env API key: none is sent,
  // and apiKeyHelper prints the key from the shell's environment.
  await host.connections.exec(
    host.endpoint,
    `${prefix}printf '%s|' "$ANTHROPIC_API_KEY" > "$HOME/seen"; sh -c ${quote(document.apiKeyHelper)} >> "$HOME/seen"`,
  );
  assert.equal(
    await fs.readFile(path.join(host.home, "seen"), "utf8"),
    "|invented-api-key",
  );
});

test("no account goes with a custom model, an empty pick, another agent, or an account with no value", async (t) => {
  const { host, cwd, handlers } = await rig(t, {
    sessions: { claudeAccount: "sub" },
  });
  const ask = (extra) =>
    handlers.get("project-session-env")({
      endpoint: host.endpoint,
      cwd,
      agent: "claude",
      ...extra,
    });
  for (const extra of [
    { modelProfileId: "m1" },
    { claudeAccountId: "" },
    { agent: "codex" },
  ]) {
    const { prefix, settings } = await ask(extra);
    assert.equal(settings, "");
    assert.equal(
      await seen(host, prefix, ["APP_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN"]),
      `${SECRET}||`,
      JSON.stringify(extra),
    );
  }
  // The project's account has no value yet: the values still arrive.
  const empty = await rig(t, { sessions: { claudeAccount: "unset" } });
  const { prefix } = await empty.handlers.get("project-session-env")({
    endpoint: empty.host.endpoint,
    cwd: empty.cwd,
    agent: "claude",
  });
  assert.equal(await seen(empty.host, prefix, ["APP_TOKEN"]), `${SECRET}|`);
});

test("a host switched off for the project gets no Codex login", async (t) => {
  const { host, cwd, handlers } = await rig(t, {
    withheld: true,
    codex: '{"tokens":"mine"}',
  });
  const answer = await handlers.get("project-session-env")({
    endpoint: host.endpoint,
    cwd,
    agent: "codex",
  });
  assert.deepEqual(answer, { prefix: "", settings: "", launch: "" });
});

test("a host switched off for the project gets no Claude account either", async (t) => {
  const { host, cwd, handlers } = await rig(t, { withheld: true });
  assert.deepEqual(
    await handlers.get("project-session-env")({
      endpoint: host.endpoint,
      cwd,
      claudeAccountId: "sub",
    }),
    { prefix: "", settings: "", launch: "" },
  );
});

test("a host with no Codex login runs a session on the local one, for that session only", async (t) => {
  const { host, cwd, handlers } = await rig(t, {
    attach: false,
    codex: '{"tokens":"mine"}',
  });
  await fs.mkdir(path.join(host.home, ".codex"));
  await fs.writeFile(path.join(host.home, ".codex", "config.toml"), "");
  const { prefix, launch } = await handlers.get("project-session-env")({
    endpoint: host.endpoint,
    cwd,
    agent: "codex",
  });
  assert.equal((prefix + launch).includes("mine"), false);
  await host.connections.exec(host.endpoint, `(${prefix}exec ${launch})`);
  const [home, auth, files] = (
    await fs.readFile(path.join(host.home, "codex-saw"), "utf8")
  ).split("|");
  assert.equal(auth, '{"tokens":"mine"}');
  assert.match(files, /config\.toml/);
  assert.match(files, /sessions/);
  // Gone with the session; the host's own Codex home never got the login.
  await assert.rejects(fs.stat(home));
  await assert.rejects(fs.stat(path.join(host.home, ".codex", "auth.json")));
});

test("a host with its own Codex login keeps using it", async (t) => {
  const { host, cwd, handlers } = await rig(t, {
    attach: false,
    codex: '{"tokens":"mine"}',
  });
  await fs.mkdir(path.join(host.home, ".codex"));
  await fs.writeFile(
    path.join(host.home, ".codex", "auth.json"),
    '{"tokens":"host"}',
  );
  const { prefix, launch } = await handlers.get("project-session-env")({
    endpoint: host.endpoint,
    cwd,
    agent: "codex",
  });
  await host.connections.exec(host.endpoint, `(${prefix}exec ${launch})`);
  const [home, auth] = (
    await fs.readFile(path.join(host.home, "codex-saw"), "utf8")
  ).split("|");
  assert.deepEqual([home, auth], ["", '{"tokens":"host"}']);
});

test("a picked Codex account runs on a host even where the host has its own login", async (t) => {
  const { host, cwd, handlers } = await rig(t, {
    attach: false,
    codex: '{"tokens":"mine"}',
  });
  await fs.mkdir(path.join(host.home, ".codex"));
  await fs.writeFile(
    path.join(host.home, ".codex", "auth.json"),
    '{"tokens":"host"}',
  );
  const { prefix, launch } = await handlers.get("project-session-env")({
    endpoint: host.endpoint,
    cwd,
    agent: "codex",
    codexAccountId: "work",
  });
  assert.equal((prefix + launch).includes("work"), false);
  await host.connections.exec(host.endpoint, `(${prefix}exec ${launch})`);
  const [home, auth] = (
    await fs.readFile(path.join(host.home, "codex-saw"), "utf8")
  ).split("|");
  assert.equal(auth, '{"tokens":"work"}');
  await assert.rejects(fs.stat(home));
  // The host's own login is left as it was.
  assert.equal(
    await fs.readFile(path.join(host.home, ".codex", "auth.json"), "utf8"),
    '{"tokens":"host"}',
  );
});

test("Codex runs as the project's account when none is picked; an empty pick is the host's own login", async (t) => {
  const { host, cwd, handlers } = await rig(t, {
    sessions: { codexAccount: "work" },
  });
  const ask = (extra) =>
    handlers.get("project-session-env")({
      endpoint: host.endpoint,
      cwd,
      agent: "codex",
      ...extra,
    });
  const own = await ask({});
  await host.connections.exec(
    host.endpoint,
    `(${own.prefix}exec ${own.launch})`,
  );
  assert.equal(
    (await fs.readFile(path.join(host.home, "codex-saw"), "utf8")).split(
      "|",
    )[1],
    '{"tokens":"work"}',
  );
  const none = await ask({ codexAccountId: "" });
  assert.equal(none.launch, "");
  assert.equal(await seen(host, none.prefix, ["SUSHIAI_CODEX_AUTH"]), "|");
  // A picked account that is not signed in is an error, the project's own
  // just falls back.
  await assert.rejects(ask({ codexAccountId: "unset" }), /Sign in/);
  const fallback = await rig(t, { sessions: { codexAccount: "unset" } });
  assert.equal(
    (
      await fallback.handlers.get("project-session-env")({
        endpoint: fallback.host.endpoint,
        cwd: fallback.cwd,
        agent: "codex",
      })
    ).launch,
    "",
  );
});

test("a local Codex pane starts in the account's own home; no login is in the text", async (t) => {
  const { handlers } = await rig(t, { attach: false });
  const answer = await handlers.get("project-session-env")({
    endpoint: "local",
    cwd: "/invented/folder",
    agent: "codex",
    codexAccountId: "work",
  });
  assert.deepEqual(answer, {
    prefix: "",
    settings: "",
    launch: "env CODEX_HOME='/invented/codex-home' codex",
  });
});

test("a remote Codex terminal runs as the picked account over the host's login, and on the local login without one", async (t) => {
  const { host, cwd, handlers, spawned } = await rig(t, {
    attach: false,
    codex: '{"tokens":"mine"}',
  });
  await fs.mkdir(path.join(host.home, ".codex"));
  const saw = async (panelId, extra, hostAuth) => {
    const own = path.join(host.home, ".codex", "auth.json");
    if (hostAuth) await fs.writeFile(own, hostAuth);
    else await fs.rm(own, { force: true });
    await handlers.get("terminal-open")({
      panelId,
      cwd,
      command: "codex",
      endpoint: host.endpoint,
      ...extra,
    });
    const command = spawned.at(-1).at(-1);
    assert.equal(command.includes("tokens"), false);
    await host.connections.exec(host.endpoint, command);
    return (await fs.readFile(path.join(host.home, "codex-saw"), "utf8")).split(
      "|",
    )[1];
  };
  assert.equal(
    await saw("r1", { codexAccountId: "work" }, '{"tokens":"host"}'),
    '{"tokens":"work"}',
  );
  assert.equal(await saw("r2", {}, ""), '{"tokens":"mine"}');
  assert.equal(await saw("r3", {}, '{"tokens":"host"}'), '{"tokens":"host"}');
});

test("a local Codex terminal runs in the account's home, and on the default login without one", async (t) => {
  const { handlers, envs, host } = await rig(t, { attach: false });
  for (const [panelId, codexAccountId] of [
    ["a", "work"],
    ["b", undefined],
  ])
    await handlers.get("terminal-open")({
      panelId,
      cwd: host.home,
      command: "codex",
      codexAccountId,
    });
  assert.equal(envs[0].CODEX_HOME, "/invented/codex-home");
  assert.equal(envs[1].CODEX_HOME, process.env.CODEX_HOME);
});

test("an agent typed with values runs in a subshell: they are gone from the pane's shell after it", async () => {
  const { agentLine } = await import("../src/workspace/workspace-actions.ts");
  assert.equal(agentLine("", "claude"), "claude");
  assert.equal(
    agentLine(". 'f'; rm -f 'f'; ", "claude", " --settings 'x'"),
    "(. 'f'; rm -f 'f'; exec claude --settings 'x')",
  );
});

test("a remote pane runs a custom model: the key in its shell, the settings inline, neither key in the text", async (t) => {
  const { modelLaunch } = require("../electron/project-session.cjs");
  const { quote } = require("../electron/connections.cjs");
  const { host, cwd, handlers } = await rig(t, { attach: false });
  const { prefix } = await handlers.get("project-session-env")({
    endpoint: host.endpoint,
    cwd,
    agent: "claude",
    modelProfileId: "m1",
  });
  const launch = await handlers.get("model-launch")("m1");
  assert.equal((prefix + launch).includes("invented-model-key"), false);
  const document = JSON.parse(
    launch.slice("claude --settings '".length, -1).replaceAll("'\\''", "'"),
  );
  assert.equal(document.env.ANTHROPIC_BASE_URL, "https://models.example.test");
  assert.equal(launch, modelLaunch(document.env));
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
