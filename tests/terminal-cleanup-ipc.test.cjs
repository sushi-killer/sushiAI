// A remote session uploads its secrets before ssh starts. Whatever stops the
// session from starting must take them off the host again.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { registerTerminalIpc } = require("../electron/ipc/terminals.cjs");
const { makeHost, makeStore } = require("./helpers/fake-host.cjs");

async function open(
  t,
  pty,
  { secondUploadFails = false, withheld = false } = {},
) {
  // mktemp into the fake HOME, so what the upload leaves is visible there.
  const host = await makeHost(t, {
    bin: { mktemp: '#!/bin/sh\nexec /usr/bin/mktemp "$HOME/tmp.XXXXXX"\n' },
  });
  // The connection drops on the second upload (the subscription token).
  const ssh = path.join(host.root, "ssh-drops");
  await fs.writeFile(
    ssh,
    `#!/bin/sh\ncase "$*" in *mktemp*)\n  n=$(cat "${host.root}/uploads" 2>/dev/null || echo 0); n=$((n+1)); echo $n > "${host.root}/uploads"\n  if [ "$n" -ge 2 ]; then cat > /dev/null; exit 255; fi ;;\nesac\nexec "${host.ssh}" "$@"\n`,
    { mode: 0o755 },
  );
  const { projects } = await makeStore(t);
  const project = await projects.upsert({
    name: "App",
    env: [{ name: "APP_TOKEN", secret: true }],
  });
  await projects.setSecret(project.id, "APP_TOKEN", "invented-session-secret");
  if (withheld) await projects.setHostWithheld(project.id, host.endpoint, true);
  const staged = [];
  const handlers = new Map();
  registerTerminalIpc({
    handle: (channel, callback) => handlers.set(channel, callback),
    send: () => {},
    app: {},
    pty,
    getConnections: () => host.connections,
    executable: (name) => `/usr/bin/${name}`,
    directory: () => {},
    id: () => {},
    terminals: new Map(),
    terminalPending: new Map(),
    stageModelSettings: async () => "",
    stageClaudeAccount: async () => {
      staged.push(1);
      const tokenPath = path.join(
        host.home,
        "..",
        `${path.basename(host.home)}-token`,
      );
      await fs.writeFile(tokenPath, "invented-subscription-token");
      return { kind: "subscription", tokenPath };
    },
    projects,
    sshBinary: secondUploadFails ? ssh : host.ssh,
  });
  const run = () =>
    handlers.get("terminal-open")({
      panelId: "panel-1",
      cwd: "/home/user/sushiai/app",
      command: "claude",
      endpoint: host.endpoint,
      projectId: project.id,
      claudeAccountId: secondUploadFails || withheld ? "account-1" : undefined,
    });
  return { host, run, staged };
}

const leftovers = async (host) => await fs.readdir(host.home);

test("a spawn that fails removes the uploaded secret file from the host", async (t) => {
  const { host, run } = await open(t, {
    spawn: () => {
      throw new Error("spawn failed");
    },
  });
  await assert.rejects(run(), /spawn failed/);
  assert.deepEqual(await leftovers(host), []);
});

test("ssh ending before the remote shell could clean up still removes the file", async (t) => {
  let exit;
  const { host, run } = await open(t, {
    spawn: () => ({
      onData: () => {},
      onExit: (callback) => {
        exit = callback;
      },
    }),
  });
  await run();
  // The upload happened and nothing on the host has consumed it yet.
  assert.equal((await leftovers(host)).length, 1);
  exit({ exitCode: 255 });
  // The cleanup is a second ssh run; wait for it.
  for (let i = 0; i < 50 && (await leftovers(host)).length; i++)
    await new Promise((resolve) => setTimeout(resolve, 50));
  assert.deepEqual(await leftovers(host), []);
});

test("a failed subscription-token upload removes the project secret file already on the host", async (t) => {
  const { host, run } = await open(
    t,
    {
      spawn: () => {
        throw new Error("must not spawn");
      },
    },
    { secondUploadFails: true },
  );
  await assert.rejects(run(), /exit|255|Command failed|ssh/i);
  assert.deepEqual(await leftovers(host), []);
});

test("a remote session's local pty carries no project values", async (t) => {
  let seen;
  const { run } = await open(t, {
    spawn: (_binary, _args, options) => {
      seen = options.env;
      return { onData: () => {}, onExit: () => {} };
    },
  });
  await run();
  assert.equal(seen.APP_TOKEN, undefined);
});

test("a host switched off for the project gets no project values and no Claude subscription token", async (t) => {
  let spawned = 0;
  const { host, run, staged } = await open(
    t,
    {
      spawn: () => {
        spawned += 1;
        return { onData: () => {}, onExit: () => {} };
      },
    },
    { withheld: true },
  );
  await run();
  assert.equal(spawned, 1);
  assert.deepEqual(staged, []);
  assert.deepEqual(await leftovers(host), []);
});
