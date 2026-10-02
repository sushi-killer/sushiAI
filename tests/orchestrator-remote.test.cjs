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

// A platform other than this machine's, so the host needs a build, not an
// upload, on any runner (a Linux x86_64 CI box included).
const FOREIGN =
  require("node:child_process")
    .execFileSync("uname", ["-sm"])
    .toString()
    .trim() === "Linux x86_64"
    ? "Linux aarch64"
    : "Linux x86_64";

const TOOLS = [
  "cat",
  "mkdir",
  "tar",
  // GNU tar execs gzip for -z; macOS tar has it built in.
  "gzip",
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
  "sh",
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
  {
    platform = "same",
    cargo = false,
    git = true,
    // A downloader that serves a fake rustup-init: "curl", "wget" or none.
    downloader = null,
    cc = false,
  } = {},
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
  if (cc)
    await fs.writeFile(path.join(bin, "cc"), "#!/bin/sh\nexit 0\n", {
      mode: 0o755,
    });
  if (downloader) {
    // The installer puts a cargo under $HOME/.cargo/bin, off the PATH.
    const template = path.join(dir, "cargo-template");
    await fs.writeFile(
      template,
      `#!/bin/sh\necho "cargo-home $@" >> "$HOME/cargo.log"\nmkdir -p target/release && cp "${binary}" target/release/orchd\n`,
      { mode: 0o755 },
    );
    const installer = `echo "rustup $*" >> "$HOME/rustup.log"; mkdir -p "$HOME/.cargo/bin"; cp "${template}" "$HOME/.cargo/bin/cargo"`;
    await fs.writeFile(
      path.join(bin, downloader),
      `#!/bin/sh\necho "${downloader} $@" >> "$HOME/fetch.log"\ncat <<'INSTALLER'\n${installer}\nINSTALLER\n`,
      { mode: 0o755 },
    );
  }
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

  async function open({ packaged = false, stopWaitSeconds } = {}) {
    // A packaged app has no orchd/ next to it: the source is a tarball among
    // the resources, next to the bundled binary.
    const resources = path.join(dir, "resources");
    if (packaged) {
      await fs.mkdir(resources, { recursive: true });
      await fs.copyFile(binary, path.join(resources, "orchd"));
      execFileSync("tar", [
        "-czf",
        path.join(resources, "orchd-src.tar.gz"),
        "--exclude",
        "target",
        "-C",
        root,
        "orchd",
      ]);
    }
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
      root: packaged ? path.join(dir, "asar") : root,
      resourcesPath: packaged ? resources : root,
      packaged,
      stopWaitSeconds,
      getConnections: () => connections,
      userDataDir: dir,
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
  const slowForward = (forwardDelayMs) =>
    fs.writeFile(
      configFile,
      JSON.stringify({ home, bin, log, forwardDelayMs }),
    );
  return { dir, home, root, remoteData, open, sshCommands, slowForward };
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
  const fx = await fixture(t, { platform: FOREIGN, cargo: true });
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

test("a packaged app builds a Linux host from its bundled source tarball", async (t) => {
  const fx = await fixture(t, { platform: FOREIGN, cargo: true });
  const { hosts, id } = await fx.open({ packaged: true });
  // No "no orchd source" error: the tarball is the source.
  const tasks = await hosts.call("task.list", {}, id);
  assert.equal(tasks[0].id, "remote-task-1");
  assert.match(
    await fs.readFile(path.join(fx.home, "cargo.log"), "utf8"),
    /cargo build --release/,
  );
  const src = path.join(fx.home, ".sushiai", "src", "orchd");
  await fs.access(path.join(src, "Cargo.toml"));
  await fs.access(path.join(src, "src", "main.rs"));
  await fs.access(path.join(fx.home, ".sushiai", "bin", "orchd"));
  // The version key is the tarball's, not a source directory's.
  const tarball = await fs.readFile(
    path.join(fx.dir, "resources", "orchd-src.tar.gz"),
  );
  assert.equal(
    await fs.readFile(
      path.join(fx.home, ".sushiai", "bin", "orchd.hash"),
      "utf8",
    ),
    require("node:crypto")
      .createHash("sha256")
      .update(tarball)
      .digest("hex")
      .slice(0, 32),
  );
});

test("a packaged app without cargo on a Linux host still says how to install Rust", async (t) => {
  const fx = await fixture(t, { platform: FOREIGN, cargo: false });
  const { hosts, id } = await fx.open({ packaged: true });
  await assert.rejects(
    hosts.call("task.list", {}, id),
    (error) =>
      error.message.includes(RUSTUP_COMMAND) &&
      !error.message.includes("no orchd source"),
  );
});

const exists = (file) =>
  fs.access(file).then(
    () => true,
    () => false,
  );

async function rejectedNeedsRust(fx) {
  const opened = await fx.open();
  await assert.rejects(
    opened.hosts.call("task.list", {}, opened.id),
    /Rust is not installed/,
  );
  return opened;
}

test("one button installs Rust with curl, builds with ~/.cargo/bin/cargo and starts orchd", async (t) => {
  const fx = await fixture(t, {
    platform: FOREIGN,
    downloader: "curl",
    cc: true,
  });
  const { hosts, id, changes } = await rejectedNeedsRust(fx);
  // Nothing is installed until the owner asks.
  assert.equal(await exists(path.join(fx.home, "rustup.log")), false);
  const preflight = await hosts.setup(id);
  assert.equal(preflight.cc, true);
  assert.match(
    await fs.readFile(path.join(fx.home, "fetch.log"), "utf8"),
    /curl --proto =https --tlsv1\.2 -sSf https:\/\/sh\.rustup\.rs/,
  );
  assert.match(
    await fs.readFile(path.join(fx.home, "rustup.log"), "utf8"),
    /rustup -y --profile minimal --no-modify-path/,
  );
  // cargo is not on the ssh PATH: the build used the one rustup installed.
  assert.match(
    await fs.readFile(path.join(fx.home, "cargo.log"), "utf8"),
    /cargo-home build --release/,
  );
  await fs.access(path.join(fx.home, ".sushiai", "bin", "orchd"));
  const host = hosts.list().find((h) => h.id === id);
  assert.equal(host.state, "ready");
  assert.ok(changes.includes("building"));
  assert.equal((await hosts.call("task.list", {}, id))[0].id, "remote-task-1");
});

test("one button falls back to wget when the host has no curl", async (t) => {
  const fx = await fixture(t, {
    platform: FOREIGN,
    downloader: "wget",
  });
  const { hosts, id } = await rejectedNeedsRust(fx);
  await hosts.setup(id);
  assert.match(
    await fs.readFile(path.join(fx.home, "fetch.log"), "utf8"),
    /wget -qO- https:\/\/sh\.rustup\.rs/,
  );
  assert.equal(hosts.list().find((h) => h.id === id).state, "ready");
});

test("one button says so when the host has neither curl nor wget, and Retry runs it again", async (t) => {
  const fx = await fixture(t, { platform: FOREIGN });
  const { hosts, id } = await rejectedNeedsRust(fx);
  await assert.rejects(
    hosts.setup(id),
    /Installing Rust on Box failed: Neither curl nor wget is installed/,
  );
  const failed = hosts.list().find((h) => h.id === id);
  assert.equal(failed.state, "error");
  assert.match(failed.detail, /^Installing Rust on Box failed:/);
  assert.equal(
    await exists(path.join(fx.home, ".cargo", "bin", "cargo")),
    false,
  );
  // A downloader appears; the same button now succeeds.
  const cargo = path.join(fx.dir, "bin", "wget");
  const orchd = path.join(fx.root, "orchd", "target", "release", "orchd");
  const installer = `mkdir -p "$HOME/.cargo/bin"; printf '#!/bin/sh\\nmkdir -p target/release && cp ${orchd} target/release/orchd\\n' > "$HOME/.cargo/bin/cargo"; chmod 755 "$HOME/.cargo/bin/cargo"`;
  await fs.writeFile(
    cargo,
    `#!/bin/sh\ncat <<'INSTALLER'\n${installer}\nINSTALLER\n`,
    { mode: 0o755 },
  );
  await hosts.setup(id);
  assert.equal(hosts.list().find((h) => h.id === id).state, "ready");
});

test("the upload plan never shows or runs the Rust install", async (t) => {
  const fx = await fixture(t, { downloader: "curl" });
  const { hosts, id } = await fx.open();
  await hosts.setup(id);
  assert.equal(hosts.list().find((h) => h.id === id).state, "ready");
  assert.equal(await exists(path.join(fx.home, "rustup.log")), false);
  assert.equal(await exists(path.join(fx.home, "fetch.log")), false);
  assert.equal(await exists(path.join(fx.home, ".cargo")), false);
});

test("one button leaves an existing cargo alone", async (t) => {
  const fx = await fixture(t, {
    platform: FOREIGN,
    cargo: true,
    downloader: "curl",
  });
  const { hosts, id } = await fx.open();
  await hosts.setup(id);
  assert.equal(await exists(path.join(fx.home, "rustup.log")), false);
  assert.match(
    await fs.readFile(path.join(fx.home, "cargo.log"), "utf8"),
    /cargo build --release/,
  );
});

test("the one-button setup rejects local and unknown hosts", async (t) => {
  const fx = await fixture(t, { platform: FOREIGN });
  const { hosts } = await fx.open();
  await assert.rejects(hosts.setup("local"), /Invalid orchestrator host/);
  await assert.rejects(hosts.setup("ssh:nope"));
  await assert.rejects(hosts.setup(7), /Invalid orchestrator host/);
});

test("preflight reports whether the host has a C linker", async (t) => {
  const withCc = await fixture(t, { cc: true });
  const a = await withCc.open();
  await a.hosts.call("task.list", {}, a.id);
  assert.equal((await a.hosts.preflight(a.id)).cc, true);
  const without = await fixture(t);
  const b = await without.open();
  await b.hosts.call("task.list", {}, b.id);
  assert.equal((await b.hosts.preflight(b.id)).cc, false);
});

test("an upgrade waits for the old daemon to exit before starting the new one", async (t) => {
  const fx = await fixture(t);
  const first = await fx.open();
  await first.hosts.call("task.list", {}, first.id);
  const oldPid = Number(
    await fs.readFile(path.join(fx.remoteData, "orchd.pid"), "utf8"),
  );
  await first.hosts.quit();
  await first.connections.close();
  // The old daemon lingers 1.5 s after it accepts the shutdown, the socket
  // forward comes up late, and a changed source makes the app plan an upgrade.
  await fs.writeFile(path.join(fx.remoteData, "exit-delay"), "1500");
  await fx.slowForward(500);
  await fs.appendFile(path.join(fx.root, "orchd", "src", "main.rs"), "// v2\n");
  const second = await fx.open();
  const tasks = await second.hosts.call("task.list", {}, second.id);
  assert.equal(tasks[0].title, "Remote job");
  assert.equal(alive(oldPid), false);
  const newPid = Number(
    await fs.readFile(path.join(fx.remoteData, "orchd.pid"), "utf8"),
  );
  assert.notEqual(newPid, oldPid);
  assert.equal(alive(newPid), true);
});

test("an upgrade fails clearly, without recording the new hash, when the old daemon will not exit", async (t) => {
  const fx = await fixture(t);
  const first = await fx.open();
  await first.hosts.call("task.list", {}, first.id);
  const oldPid = Number(
    await fs.readFile(path.join(fx.remoteData, "orchd.pid"), "utf8"),
  );
  const hashFile = path.join(fx.home, ".sushiai", "bin", "orchd.hash");
  const oldHash = await fs.readFile(hashFile, "utf8");
  await first.hosts.quit();
  await first.connections.close();
  await fs.writeFile(path.join(fx.remoteData, "exit-delay"), "60000");
  await fs.appendFile(path.join(fx.root, "orchd", "src", "main.rs"), "// v2\n");
  const second = await fx.open({ stopWaitSeconds: 1 });
  await assert.rejects(
    second.hosts.call("task.list", {}, second.id),
    /did not stop within 1 s/,
  );
  assert.equal(await fs.readFile(hashFile, "utf8"), oldHash);
  assert.equal(alive(oldPid), true);
});

test("a different platform without cargo says how to install Rust", async (t) => {
  const fx = await fixture(t, { platform: FOREIGN, cargo: false });
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
  assert.equal(failed.platform, FOREIGN);
  assert.equal(failed.orchdInstalled, false);
  for (
    let i = 0;
    i < 100 && !hosts.list().find((h) => h.id === id).preflight;
    i++
  )
    await new Promise((resolve) => setTimeout(resolve, 50));
  assert.equal(hosts.list().find((h) => h.id === id).preflight.git, true);
  // Asking again right away fails fast with the same error and no ssh; an
  // explicit recheck (Try again) probes the host once more.
  const probes = async () =>
    (await fx.sshCommands()).filter((c) => JSON.stringify(c).includes("uname"))
      .length;
  const before = await probes();
  await assert.rejects(
    hosts.call("task.list", {}, id),
    /Rust is not installed/,
  );
  assert.equal(await probes(), before);
  await assert.rejects(hosts.preflight(id), /Rust is not installed/);
  assert.equal(await probes(), before + 1);
  // Nothing was left half-installed.
  await assert.rejects(
    fs.access(path.join(fx.home, ".sushiai", "bin", "orchd")),
  );
});

test("an up-to-date install is reused, quitting the app leaves the remote daemon running, and a saved host connects on first use", async (t) => {
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

  // Reopening knows the enabled host but touches nothing until it is used.
  const second = await fx.open();
  await second.hosts.init();
  await new Promise((resolve) => setTimeout(resolve, 200));
  assert.equal((await fx.sshCommands()).length, before);
  assert.equal(
    second.hosts.list().find((h) => h.id === second.id).enabled,
    true,
  );
  const tasks = await second.hosts.call("task.list", {}, second.id);
  await waitUntil(() => second.events.some((e) => e.message.event === "task"));
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
    cc: false,
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
