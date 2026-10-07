const fs = require("node:fs/promises");
const path = require("node:path");
const { spawn } = require("node:child_process");
const net = require("node:net");
const { randomUUID } = require("node:crypto");
const { appDb, transaction } = require("./app-db.cjs");
const { InspectionWorker } = require("./inspection-worker.cjs");
const quote = (text) => "'" + String(text).replaceAll("'", "'\\''") + "'";

function run(binary, args, input = "", timeout = 20000) {
  return new Promise((resolve, reject) => {
    const proc = spawn(binary, args, { stdio: ["pipe", "pipe", "pipe"] });
    let stdout = "",
      stderr = "",
      failure;
    const timer = setTimeout(() => {
      failure = Object.assign(new Error("Connection timed out"), {
        timedOut: true,
      });
      proc.kill();
    }, timeout);
    proc.stdout.setEncoding("utf8");
    proc.stderr.setEncoding("utf8");
    proc.stdout.on("data", (chunk) => {
      stdout += chunk;
      if (stdout.length > 24 * 1024 * 1024) {
        failure = new Error("Response exceeds 24 MB");
        proc.kill();
      }
    });
    proc.stderr.on("data", (chunk) => {
      stderr = (stderr + chunk).slice(-4000);
    });
    proc.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    proc.on("close", (code) => {
      clearTimeout(timer);
      failure
        ? reject(Object.assign(failure, { stderr }))
        : code
          ? reject(new Error(stderr || `Process exited (${code})`))
          : resolve(stdout);
    });
    proc.stdin.on("error", () => {});
    proc.stdin.end(input);
  });
}
/** Like run(), but for a caller that wants the exit code and the end of both
 * streams (at most `tailBytes` each, kept as raw bytes) whatever the code is.
 * A timeout kills the process and reports code null. */
function runTail(binary, args, input, timeout, tailBytes) {
  return new Promise((resolve, reject) => {
    const proc = spawn(binary, args, { stdio: ["pipe", "pipe", "pipe"] });
    let out = Buffer.alloc(0);
    let err = Buffer.alloc(0);
    let timedOut = false;
    const keep = (buffer, chunk) =>
      Buffer.concat([buffer, chunk]).subarray(-tailBytes);
    const timer = setTimeout(() => {
      timedOut = true;
      proc.kill("SIGKILL");
    }, timeout);
    proc.stdout.on("data", (chunk) => (out = keep(out, chunk)));
    proc.stderr.on("data", (chunk) => (err = keep(err, chunk)));
    proc.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    proc.on("close", (code) => {
      clearTimeout(timer);
      resolve({
        code: timedOut ? null : code,
        stdout: out.toString("utf8"),
        stderr: timedOut
          ? `${err.toString("utf8")}\nTimed out after ${timeout} ms.`.trim()
          : err.toString("utf8"),
      });
    });
    proc.stdin.on("error", () => {});
    proc.stdin.end(input);
  });
}
// `ssh -G` prints the resolved config as one lowercase "key value" per line.
// Anchored on purpose: forwardagent, forwardx11, exitonforwardfailure and
// clearallforwardings are printed for every host and are not forwards.
function declaresForwards(config) {
  return /^(localforward|remoteforward|dynamicforward)\s/m.test(config);
}
const MAX_ARGV = 64;
const MAX_ARG_LENGTH = 4096;
/** How the desktop reaches the sushiai daemon: ssh (the default, stored as
 * nothing) or a local command that speaks the daemon protocol on stdio. */
function validateConnector(connector) {
  if (connector === undefined || connector === null) return undefined;
  if (typeof connector !== "object" || Array.isArray(connector))
    throw new Error("Invalid connector.");
  if (connector.kind === "ssh") return { kind: "ssh" };
  if (connector.kind !== "command")
    throw new Error("The connector must be ssh or a command.");
  const { argv } = connector;
  if (
    !Array.isArray(argv) ||
    !argv.length ||
    argv.length > MAX_ARGV ||
    argv.some(
      (arg) =>
        typeof arg !== "string" ||
        arg.length > MAX_ARG_LENGTH ||
        arg.includes("\0"),
    ) ||
    !argv[0].trim()
  )
    throw new Error("Enter the command that connects to the daemon.");
  return { kind: "command", argv: [...argv] };
}
function validate(profile) {
  // A command connector never runs ssh, so its host is only a label.
  const commandHost =
    profile?.connector?.kind === "command" && !profile.host
      ? "command"
      : profile?.host;
  if (!profile || !/^[a-zA-Z0-9_][a-zA-Z0-9_.@-]*$/.test(commandHost || ""))
    throw new Error("Enter an SSH alias or user@hostname.");
  if (
    profile.port &&
    (!Number.isInteger(Number(profile.port)) ||
      Number(profile.port) < 1 ||
      Number(profile.port) > 65535)
  )
    throw new Error("Invalid SSH port");
  const connector = validateConnector(profile.connector);
  return {
    id: /^[a-f0-9-]{36}$/.test(profile.id || "") ? profile.id : randomUUID(),
    name: String(profile.name || commandHost).slice(0, 80),
    host: commandHost,
    port: Number(profile.port) || undefined,
    ...(connector ? { connector } : {}),
    hidden: Boolean(profile.hidden),
    autoConnect: Boolean(profile.autoConnect),
  };
}
class Connections {
  // `ssh` is the binary every connection runs; tests hand in a fake one.
  constructor(dataDir, { ssh = "/usr/bin/ssh" } = {}) {
    this.ssh = ssh;
    this.dataDir = dataDir;
    this.knownHostsFile = path.join(dataDir, "known_hosts");
    this.profiles = [];
    this.inspectionWorkers = new Map();
    this.inspectionSourcePromise = null;
    this.profileListeners = new Set();
    this.forwards = new Map();
    this.closed = false;
    this.closePromise = null;
  }
  async init() {
    this.temp = await fs.mkdtemp("/tmp/sushiai-ssh-");
    this.profiles = [];
    const rows = appDb(this.dataDir)
      .prepare("SELECT data FROM connections ORDER BY position")
      .all();
    for (const row of rows) {
      try {
        this.profiles.push(validate(JSON.parse(row.data)));
      } catch (error) {
        console.warn(`Skipping a saved connection: ${error.message}`);
      }
    }
    this.profilesChanged();
  }
  /** Calls `listener(profiles)` after the profile list changed (loaded, saved
   * or deleted); the daemon manager builds its connectors from it. */
  onProfilesChange(listener) {
    this.profileListeners.add(listener);
    return () => this.profileListeners.delete(listener);
  }
  profilesChanged() {
    for (const listener of this.profileListeners) {
      try {
        listener(this.profiles);
      } catch (error) {
        console.warn(`Connection listener failed: ${error.message}`);
      }
    }
  }
  /** False for a host reached through a command connector: there is no ssh
   * shell to run commands or inspections on. */
  hasShell(endpoint) {
    return this.get(endpoint).connector?.kind !== "command";
  }
  #needShell(endpoint) {
    if (!this.hasShell(endpoint))
      throw new Error(
        "This host connects through a command and has no shell access.",
      );
  }
  /** Rewrites every profile row in one transaction. */
  persist() {
    const db = appDb(this.dataDir);
    transaction(db, () => {
      db.exec("DELETE FROM connections");
      const insert = db.prepare(
        "INSERT INTO connections(id, position, data) VALUES(?, ?, ?)",
      );
      this.profiles.forEach((profile, position) =>
        insert.run(profile.id, position, JSON.stringify(profile)),
      );
    });
  }
  list() {
    return this.profiles.map((p) => ({ ...p }));
  }
  get(endpoint) {
    const p = this.profiles.find((p) => `ssh:${p.id}` === endpoint);
    if (!p) throw new Error("SSH connection not found. Add it in Settings.");
    return p;
  }
  args(profile) {
    return [
      "-T",
      "-o",
      "BatchMode=yes",
      // A user's own ssh_config can set UserKnownHostsFile=/dev/null (common
      // alongside their own StrictHostKeyChecking=no), which would silently
      // discard every host key we try to save. Point at our own file instead
      // of trusting whatever the user's global config resolves to, so a new
      // host's key is actually remembered and later changes are still
      // caught - `accept-new` trusts a host's key the first time (there is
      // no UI to pre-approve one), then verifies it on every connection after.
      "-o",
      `UserKnownHostsFile=${this.knownHostsFile}`,
      "-o",
      "StrictHostKeyChecking=accept-new",
      "-o",
      "ConnectTimeout=8",
      "-o",
      "ServerAliveInterval=15",
      "-o",
      "ServerAliveCountMax=2",
      ...(profile.port ? ["-p", String(profile.port)] : []),
    ];
  }
  async save(profile) {
    const p = validate(profile);
    this.profiles = [...this.profiles.filter((x) => x.id !== p.id), p];
    await this.disconnect(`ssh:${p.id}`);
    this.persist();
    this.profilesChanged();
    return p;
  }
  /** Hides a profile from the workspace sidebar without touching its tunnel -
   * unlike `save`, this never disconnects, since it changes only how the app
   * displays the connection, not the connection itself. */
  async setHidden(endpoint, hidden) {
    const p = this.get(endpoint);
    const next = { ...p, hidden: Boolean(hidden) };
    this.profiles = this.profiles.map((x) => (x.id === p.id ? next : x));
    this.persist();
    return next;
  }
  /** Tracks whether this profile should reconnect on the next app launch -
   * flipped on by an explicit Connect, off by an explicit Disconnect, so the
   * app resumes whatever the user last left running without asking again. */
  async setAutoConnect(endpoint, autoConnect) {
    const p = this.get(endpoint);
    const next = { ...p, autoConnect: Boolean(autoConnect) };
    this.profiles = this.profiles.map((x) => (x.id === p.id ? next : x));
    this.persist();
    return next;
  }
  async delete(endpoint) {
    const p = this.get(endpoint);
    this.profiles = this.profiles.filter((x) => x.id !== p.id);
    await this.disconnect(endpoint);
    this.persist();
    this.profilesChanged();
  }
  async inspect(endpoint, options) {
    if (this.closed) throw new Error("Connections are closed.");
    if (endpoint?.startsWith("ssh:")) this.#needShell(endpoint);
    return this.inspectionWorker(endpoint).request(options);
  }
  async inspectionSource() {
    if (!this.inspectionSourcePromise) {
      this.inspectionSourcePromise = fs
        .readFile(path.join(__dirname, "remote-files.py"), "utf8")
        .catch((error) => {
          this.inspectionSourcePromise = null;
          throw error;
        });
    }
    return this.inspectionSourcePromise;
  }
  inspectionWorker(endpoint) {
    const profile = endpoint?.startsWith("ssh:") ? this.get(endpoint) : null;
    const key = profile ? `ssh:${profile.id}` : "local";
    let worker = this.inspectionWorkers.get(key);
    if (worker) return worker;
    worker = new InspectionWorker({
      command: profile ? this.ssh : "/usr/bin/python3",
      args: (source) =>
        profile
          ? [
              ...this.args(profile),
              profile.host,
              `python3 -u -c ${quote(source)} --sushiai-worker`,
            ]
          : ["-u", "-c", source, "--sushiai-worker"],
      sourceLoader: () => this.inspectionSource(),
      label: profile
        ? `Project inspection for ${profile.name}`
        : "Project inspection",
    });
    this.inspectionWorkers.set(key, worker);
    return worker;
  }
  /** Runs one shell command on the host over ssh (`input` goes to its
   * stdin) and resolves with its stdout. */
  exec(endpoint, command, { input = "", timeout = 20000 } = {}) {
    const profile = this.get(endpoint);
    this.#needShell(endpoint);
    return run(
      this.ssh,
      [...this.args(profile), profile.host, command],
      input,
      timeout,
    );
  }
  /** Runs argv on the host over the same ssh path as exec(); resolves with
   * {code, stdout, stderr} (the last `tailBytes` of each) for any exit code.
   * The words are quoted for the remote shell. `input` is a Buffer or string. */
  async execArgv(
    endpoint,
    argv,
    { input = "", timeout = 120000, tailBytes = 65536 } = {},
  ) {
    const profile = this.get(endpoint);
    this.#needShell(endpoint);
    return runTail(
      this.ssh,
      [...this.args(profile), profile.host, argv.map(quote).join(" ")],
      input,
      timeout,
      tailBytes,
    );
  }
  /** Starts `ssh -L specification` (through the shared master when it has
   * no competing forward); the returned handle emits `exit` when the tunnel
   * drops and `kill()` closes it. */
  async forwardProcess(profile, specification) {
    // A LocalForward/RemoteForward the user's own ssh_config declares for this
    // Host rides along on every ssh we spawn, and ssh cannot keep our -L while
    // dropping theirs - ClearAllForwardings clears both. On a shared master the
    // competing forward can fail the -O forward after ours is registered,
    // stranding ours there. So if the config declares any, go private. A
    // failed `ssh -G` also lands here: private is the path that always works.
    let shared = false;
    try {
      const config = await run(
        this.ssh,
        [...this.args(profile), "-G", profile.host],
        "",
        3000,
      );
      if (!declaresForwards(config)) {
        await run(
          this.ssh,
          [...this.args(profile), "-O", "check", profile.host],
          "",
          3000,
        );
        shared = true;
      }
    } catch {}
    if (shared) {
      try {
        await run(this.ssh, [
          ...this.args(profile),
          "-O",
          "forward",
          "-L",
          specification,
          profile.host,
        ]);
        const { EventEmitter } = require("node:events");
        const handle = new EventEmitter();
        handle.stderr = new EventEmitter();
        handle.shared = true;
        let closed = false;
        handle.kill = async () => {
          if (closed) return;
          closed = true;
          await run(
            this.ssh,
            [
              ...this.args(profile),
              "-O",
              "cancel",
              "-L",
              specification,
              profile.host,
            ],
            "",
            3000,
          ).catch(() => {});
          handle.emit("exit");
        };
        return handle;
      } catch {
        // The shared master refused this forward - e.g. a LocalForward the
        // user's own ssh_config declares for this Host competing for the
        // same local port. Fall through to a private connection instead of
        // failing outright: it isn't subject to that master's bindings.
      }
    }
    return spawn(
      this.ssh,
      [
        ...this.args(profile),
        "-o",
        "ControlMaster=no",
        "-o",
        "ControlPath=none",
        "-N",
        // Not yes: a LocalForward the user's ssh_config adds for this Host, on
        // a port their own session already holds, would kill this connection
        // even though our own -L came up. Both callers probe their forward by
        // actually connecting to it, so a failure that matters still surfaces.
        "-o",
        "ExitOnForwardFailure=no",
        "-L",
        specification,
        profile.host,
      ],
      { stdio: ["ignore", "ignore", "pipe"] },
    );
  }
  /** The live port forwards of a profile: remote port -> {port, proc}. */
  forwardsFor(id) {
    if (!this.forwards.has(id)) this.forwards.set(id, new Map());
    return this.forwards.get(id);
  }
  async forward(endpoint, port) {
    if (!Number.isInteger(port) || port < 1 || port > 65535)
      throw new Error("Invalid port");
    const p = this.get(endpoint);
    this.#needShell(endpoint);
    const state = { forwards: this.forwardsFor(p.id) };
    if (state.forwards.has(port)) return state.forwards.get(port).port;
    const reservation = net.createServer();
    await new Promise((resolve) => reservation.listen(0, "127.0.0.1", resolve));
    const local = reservation.address().port;
    await new Promise((resolve) => reservation.close(resolve));
    const proc = await this.forwardProcess(
      p,
      `127.0.0.1:${local}:127.0.0.1:${port}`,
    );
    let error = "",
      exited = false;
    proc.stderr.on("data", (d) => {
      error += d;
    });
    proc.on("error", (e) => {
      error = e.message;
      exited = true;
    });
    proc.on("exit", () => {
      exited = true;
      if (state.forwards.get(port)?.proc === proc) state.forwards.delete(port);
    });
    for (let i = 0; i < 40 && !exited; i++) {
      await new Promise((r) => setTimeout(r, 150));
      const ready = await new Promise((resolve) => {
        const c = net.createConnection({ host: "127.0.0.1", port: local });
        c.on("connect", () => {
          c.destroy();
          resolve(true);
        });
        c.on("error", () => resolve(false));
      });
      if (ready) {
        state.forwards.set(port, { port: local, proc });
        return local;
      }
    }
    await proc.kill();
    throw new Error(error || "Port forwarding failed");
  }
  async disconnect(endpoint) {
    const workerKey = endpoint?.startsWith("ssh:") ? endpoint : "local";
    const inspector = this.inspectionWorkers.get(workerKey);
    this.inspectionWorkers.delete(workerKey);
    if (inspector) await inspector.close("Connection disconnected.");
    if (!endpoint?.startsWith("ssh:")) return;
    const id = endpoint.replace(/^ssh:/, "");
    const forwards = this.forwards.get(id);
    this.forwards.delete(id);
    for (const forward of forwards?.values() ?? []) await forward.proc.kill();
  }
  async close() {
    if (this.closePromise) return this.closePromise;
    this.closed = true;
    this.closePromise = (async () => {
      const inspectors = [...this.inspectionWorkers.values()];
      this.inspectionWorkers.clear();
      await Promise.allSettled(inspectors.map((worker) => worker.close()));
      for (const id of [...this.forwards.keys()])
        await this.disconnect(`ssh:${id}`);
      if (this.temp) await fs.rm(this.temp, { recursive: true, force: true });
    })();
    return this.closePromise;
  }
}
module.exports = { Connections, run, quote, validate, declaresForwards };
