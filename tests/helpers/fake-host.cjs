// A fake SSH host for tests: a real Connections object whose ssh runs the
// "remote" command locally under a fake HOME and a controlled PATH (see
// tests/fixtures/fake-ssh.cjs). `bin` adds or replaces tools on that PATH.
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const { Connections } = require("../../electron/connections.cjs");

const TOOLS =
  "cat mkdir tar mv cp chmod rm uname kill nohup sleep dirname setsid git sed shasum sha256sum mktemp find head tr cut printf env sh ls grep awk timeout date base64 touch test cksum".split(
    " ",
  );

async function makeHost(t, { bin = {} } = {}) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "fake-host-"));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const home = path.join(root, "home");
  const tools = path.join(root, "bin");
  await fs.mkdir(home);
  await fs.mkdir(tools);
  const realGit = execFileSync("/bin/sh", ["-c", "command -v git"], {
    encoding: "utf8",
  }).trim();
  for (const name of TOOLS) {
    try {
      const real = execFileSync("/bin/sh", ["-c", `command -v ${name}`], {
        encoding: "utf8",
      }).trim();
      if (real && !(name in bin))
        await fs.symlink(real, path.join(tools, name));
    } catch {}
  }
  for (const [name, body] of Object.entries(bin))
    await fs.writeFile(
      path.join(tools, name),
      body.replaceAll("REAL_GIT", realGit),
      {
        mode: 0o755,
      },
    );
  const log = path.join(root, "ssh.log");
  await fs.writeFile(log, "");
  const config = path.join(root, "ssh.json");
  await fs.writeFile(config, JSON.stringify({ home, bin: tools, log }));
  const ssh = path.join(root, "ssh");
  await fs.writeFile(
    ssh,
    `#!${process.execPath}\nprocess.env.FAKE_SSH_CONFIG=${JSON.stringify(config)};\nrequire(${JSON.stringify(path.join(__dirname, "../fixtures/fake-ssh.cjs"))});\n`,
    { mode: 0o755 },
  );
  const connections = new Connections(root, { ssh });
  const profile = await connections.save({
    id: "00000000-0000-4000-8000-0000000000aa",
    name: "Devbox",
    host: "user@devbox.example.test",
    socket: "~/.sushiai/orchestrator/orchd.sock",
  });
  return {
    root,
    home,
    log,
    ssh,
    connections,
    endpoint: `ssh:${profile.id}`,
  };
}

/** A project store over a temp folder with an in-memory safeStorage. */
async function makeStore(t) {
  const { Projects } = require("../../electron/projects.cjs");
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "project-store-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const safeStorage = {
    isEncryptionAvailable: () => true,
    getSelectedStorageBackend: () => "keychain",
    encryptString: (value) => Buffer.from(`enc:${value}`),
    decryptString: (value) => value.toString().replace(/^enc:/, ""),
  };
  return { projects: new Projects({ userDataDir: dir, safeStorage }), dir };
}

/** The project IPC handlers, registered over a store and connections. */
function registerHandlers({ projects, connections, claudeMcp }) {
  const { registerProjectIpc } = require("../../electron/ipc/projects.cjs");
  const handlers = new Map();
  registerProjectIpc({
    handle: (channel, callback) => handlers.set(channel, callback),
    projects,
    getConnections: () => connections,
    getClaudeMcp: () => claudeMcp,
    getPreview: () => ({}),
    terminals: new Map(),
    terminalPending: new Map(),
  });
  return (channel, ...args) => handlers.get(channel)(...args);
}

module.exports = { makeHost, makeStore, registerHandlers };
