const { test } = require("node:test");
const assert = require("node:assert/strict");
const path = require("node:path");
const fs = require("node:fs/promises");
const { execFileSync } = require("node:child_process");
const { Connections } = require("../electron/connections.cjs");
const {
  createOrchestratorHosts,
  orchestratorNotice,
} = require("../electron/orchestrator.cjs");
const {
  parsePreflight,
  installPlan,
  RUSTUP_COMMAND,
} = require("../electron/orchestrator-remote.cjs");

const TOOLS = [
  "cat",
  "mkdir",
  "tar",
  "mv",
  "cp",
  "chmod",
  "rm",
  "uname",
  "kill",
  "nohup",
  "sleep",
  "dirname",
  "setsid",
];

async function waitUntil(check, { timeout = 8000, interval = 25 } = {}) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await check()) return;
    await new Promise((resolve) => setTimeout(resolve, interval));
  }
  throw new Error("waitUntil: condition never became true");
}

function realTool(name) {
  try {
    return execFileSync("/bin/sh", ["-c", `command -v ${name}`], {
      encoding: "utf8",
    }).trim();
  } catch {
    return "";
  }
}

/** A fake host: a HOME, a PATH of symlinked real tools (plus whatever the
 * scenario adds), a fake ssh and a local repo whose `orchd/` the app ships. */
async function fixture(
  t,
  { platform = "same", cargo = false, git = true } = {},
) {
  const dir = await fs.mkdtemp("/tmp/orch-");
  const home = path.join(dir, "home");
  const bin = path.join(dir, "bin");
  const root = path.join(dir, "app");
  await fs.mkdir(home);
  await fs.mkdir(bin);
  for (const name of TOOLS.concat(git ? ["git"] : [])) {
    if (name === "uname" && platform !== "same") continue;
    const real = realTool(name);
    if (real) await fs.symlink(real, path.join(bin, name));
  }
  const fakeDaemon = await fs.readFile(
    path.join(__dirname, "fixtures", "fake-orchd.cjs"),
    "utf8",
  );
  const binary = path.join(root, "orchd", "target", "release", "orchd");
  await fs.mkdir(path.dirname(binary), { recursive: true });
  await fs.mkdir(path.join(root, "orchd", "src"));
  await fs.writeFile(path.join(root, "orchd", "Cargo.toml"), "[package]\n");
  await fs.writeFile(
    path.join(root, "orchd", "src", "main.rs"),
    "fn main() {}\n",
  );
  await fs.writeFile(binary, `#!${process.execPath}\n${fakeDaemon}`, {
    mode: 0o755,
  });
  if (platform !== "same")
    await fs.writeFile(
      path.join(bin, "uname"),
      `#!/bin/sh\necho "${platform}"\n`,
      { mode: 0o755 },
    );
  if (cargo)
    await fs.writeFile(
      path.join(bin, "cargo"),
      `#!/bin/sh\necho "cargo $@" >> "$HOME/cargo.log"\nmkdir -p target/release && cp "${binary}" target/release/orchd\n`,
      { mode: 0o755 },
    );
  const log = path.join(dir, "ssh.log");
  await fs.writeFile(log, "");
  const configFile = path.join(dir, "ssh.json");
  await fs.writeFile(configFile, JSON.stringify({ home, bin, log }));
  process.env.FAKE_SSH_CONFIG = configFile;
  const ssh = path.join(dir, "ssh");
  await fs.writeFile(
    ssh,
    `#!${process.execPath}\nrequire(${JSON.stringify(path.join(__dirname, "fixtures", "fake-ssh.cjs"))});\n`,
    { mode: 0o755 },
  );
  const remoteData = path.join(home, ".sushiai", "orchestrator");
  const opened = [];
  t.after(async () => {
    if (process.env.KEEP_FIXTURE) console.log("fixture", dir);
    for (const hosts of opened) await hosts.quit().catch(() => {});
    try {
      const pid = Number(
        await fs.readFile(path.join(remoteData, "orchd.pid"), "utf8"),
      );
      process.kill(pid, "SIGKILL");
    } catch {
      // No daemon was started.
    }
    if (!process.env.KEEP_FIXTURE)
      await fs.rm(dir, {
        recursive: true,
        force: true,
        maxRetries: 10,
        retryDelay: 50,
      });
  });

  async function open() {
    const connections = new Connections(path.join(dir, "userdata"), { ssh });
    await connections.init();
    const profile =
      connections.profiles[0] ??
      (await connections.save({
        host: "box",
        name: "Box",
        socket: "~/.herdr.sock",
      }));
    const events = [];
    const notices = [];
    const changes = [];
    const hosts = createOrchestratorHosts({
      send: (channel, message) => events.push({ channel, message }),
      notify: (notice) => notices.push(notice),
      dataDir: path.join(dir, "local-data"),
      root,
      resourcesPath: root,
      packaged: false,
      getConnections: () => connections,
      hostsFile: path.join(dir, "hosts.json"),
      hostsChanged: () => changes.push(hosts.list().at(-1).state),
      spawnRetries: 60,
      spawnIntervalMs: 50,
    });
    opened.push({
      quit: async () => {
        await hosts.quit();
        await connections.close();
      },
    });
    return {
      hosts,
      connections,
      events,
      notices,
      changes,
      id: `ssh:${profile.id}`,
    };
  }
  const sshCommands = async () =>
    (await fs.readFile(log, "utf8"))
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line));
  return { dir, home, remoteData, open, sshCommands };
}

const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

test("same platform: uploads the local binary, starts it detached and connects with the remote token", async (t) => {
  const fx = await fixture(t);
  const { hosts, events, notices, id } = await fx.open();
  const tasks = await hosts.call("task.list", {}, id);
  // Through the forwarded socket, authenticated with the token read over ssh.
  assert.equal(tasks[0].title, "Remote job");
  assert.equal(tasks[0].host, id);
  const installed = path.join(fx.home, ".sushiai", "bin", "orchd");
  await fs.access(installed);
  assert.equal((await fs.stat(installed)).mode & 0o111, 0o111);
  assert.match(
    await fs.readFile(installed + ".hash", "utf8"),
    /^[0-9a-f]{32}$/,
  );
  // The token file itself stays 0600 on the host.
  assert.equal(
    (await fs.stat(path.join(fx.remoteData, "control.token"))).mode & 0o777,
    0o600,
  );
  // No cargo involved.
  const commands = (await fx.sshCommands()).map((argv) => argv.join(" "));
  assert.equal(
    commands.some((line) => line.includes("cargo build")),
    false,
  );
  // The subscribe stream relays the remote task, tagged with its host, and
  // raises the notice with the host so opening it can select the host.
  await waitUntil(() => events.some((e) => e.message.event === "task"));
  const event = events.find((e) => e.message.event === "task");
  assert.equal(event.message.host, id);
  assert.equal(event.message.task.host, id);
  assert.equal(notices.length, 1);
  assert.equal(notices[0].kind, "input");
  assert.equal(notices[0].host, id);
  assert.equal(notices[0].taskId, "remote-task-1");
  assert.deepEqual(
    hosts.list().map((h) => [h.id, h.state]),
    [
      ["local", "ready"],
      [id, "ready"],
    ],
  );
});

test("a different platform builds from the source with cargo on the host", async (t) => {
  const fx = await fixture(t, { platform: "Linux x86_64", cargo: true });
  const { hosts, id } = await fx.open();
  const tasks = await hosts.call("task.list", {}, id);
  assert.equal(tasks[0].id, "remote-task-1");
  assert.match(
    await fs.readFile(path.join(fx.home, "cargo.log"), "utf8"),
    /cargo build --release/,
  );
  // The archive carried the source but not `target/`.
  const src = path.join(fx.home, ".sushiai", "src", "orchd");
  await fs.access(path.join(src, "Cargo.toml"));
  await fs.access(path.join(src, "src", "main.rs"));
  await fs.access(path.join(src, "target", "release", "orchd"));
  await fs.access(path.join(fx.home, ".sushiai", "bin", "orchd"));
});

test("a different platform without cargo says how to install Rust", async (t) => {
  const fx = await fixture(t, { platform: "Linux aarch64", cargo: false });
  const { hosts, id } = await fx.open();
  await assert.rejects(
    hosts.call("task.list", {}, id),
    (error) =>
      error.message.includes("Rust is not installed on Box") &&
      error.message.includes(RUSTUP_COMMAND),
  );
  const failed = hosts.list().find((h) => h.id === id);
  assert.equal(failed.state, "error");
  // The probe's facts and the preflight reach the setup card even though
  // orchd never started.
  assert.equal(failed.platform, "Linux aarch64");
  assert.equal(failed.orchdInstalled, false);
  for (
    let i = 0;
    i < 100 && !hosts.list().find((h) => h.id === id).preflight;
    i++
  )
    await new Promise((resolve) => setTimeout(resolve, 50));
  assert.equal(hosts.list().find((h) => h.id === id).preflight.git, true);
  // Nothing was left half-installed.
  await assert.rejects(
    fs.access(path.join(fx.home, ".sushiai", "bin", "orchd")),
  );
});

test("an up-to-date install is reused and quitting the app leaves the daemon running", async (t) => {
  const fx = await fixture(t);
  const first = await fx.open();
  await first.hosts.call("task.list", {}, first.id);
  const pid = Number(
    await fs.readFile(path.join(fx.remoteData, "orchd.pid"), "utf8"),
  );
  assert.equal(alive(pid), true);
  const before = (await fx.sshCommands()).length;
  await first.hosts.quit();
  await first.connections.close();
  // The app is gone; the daemon is not.
  await new Promise((resolve) => setTimeout(resolve, 200));
  assert.equal(alive(pid), true);

  // Reopening reconnects to the enabled host on its own and shows its tasks.
  const second = await fx.open();
  await second.hosts.init();
  await waitUntil(() => second.events.some((e) => e.message.event === "task"));
  const tasks = await second.hosts.call("task.list", {}, second.id);
  assert.equal(tasks[0].title, "Remote job");
  const commands = (await fx.sshCommands())
    .slice(before)
    .map((argv) => argv.join(" "));
  assert.equal(
    commands.some((line) => line.includes("orchd.new")),
    false,
  );
  assert.equal(
    await fs.readFile(path.join(fx.remoteData, "orchd.pid"), "utf8"),
    String(pid),
  );
});

test("reconnects when the ssh forward drops", async (t) => {
  const fx = await fixture(t);
  const { hosts, id } = await fx.open();
  await hosts.call("task.list", {}, id);
  const remote = hosts.services.get(id).remote;
  remote.conn.proc.kill();
  await waitUntil(() => remote.conn === null);
  const tasks = await hosts.call("task.list", {}, id);
  assert.equal(tasks[0].title, "Remote job");
  assert.equal(remote.state, "ready");
});

test("preflight reports git, claude and codex per host", async (t) => {
  const fx = await fixture(t, { git: false });
  const claude = path.join(fx.dir, "bin", "claude");
  await fs.writeFile(claude, "#!/bin/sh\nexit 0\n", { mode: 0o755 });
  const codex = path.join(fx.dir, "bin", "codex");
  await fs.writeFile(codex, "#!/bin/sh\nexit 1\n", { mode: 0o755 });
  const { hosts, id } = await fx.open();
  await hosts.call("task.list", {}, id);
  const preflight = await hosts.preflight(id);
  assert.equal(preflight.git, false);
  assert.deepEqual(preflight.claude, { installed: true, loggedIn: true });
  assert.deepEqual(preflight.codex, { installed: true, loggedIn: false });
  assert.deepEqual(hosts.list().find((h) => h.id === id).preflight, preflight);
});

test("parsePreflight and installPlan read the probe output", () => {
  assert.deepEqual(parsePreflight("git=1\nclaude=1\nclaude_login=1\n", 5), {
    git: true,
    claude: { installed: true, loggedIn: true },
    codex: { installed: false, loggedIn: false },
    checkedAt: 5,
  });
  const want = { hash: "aa", binary: "/b", platform: "Darwin arm64" };
  assert.equal(
    installPlan({ installed: "1", hash: "aa", platform: "x" }, want),
    "none",
  );
  assert.equal(
    installPlan(
      { installed: "1", hash: "old", platform: "Darwin arm64" },
      want,
    ),
    "upload",
  );
  assert.equal(installPlan({ platform: "Linux x86_64" }, want), "build");
  assert.equal(
    installPlan({ platform: "Darwin arm64" }, { ...want, binary: null }),
    "build",
  );
});

test("a notice for a remote task names its host; a local one does not", () => {
  const task = { id: "t", repo: "/r", title: "x", status: "waiting" };
  assert.equal("host" in orchestratorNotice(task), false);
  assert.equal(orchestratorNotice({ ...task, host: "local" }).host, undefined);
  assert.equal(
    orchestratorNotice({ ...task, host: "ssh:abc" }).host,
    "ssh:abc",
  );
});
