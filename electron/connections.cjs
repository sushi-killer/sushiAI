const fs = require("node:fs/promises");
const path = require("node:path");
const { spawn } = require("node:child_process");
const net = require("node:net");
const { randomUUID } = require("node:crypto");
const { request } = require("./herdr.cjs");
const quote = (text) => "'" + String(text).replaceAll("'", "'\\''") + "'";

function run(binary, args, input = "", timeout = 20000) {
  return new Promise((resolve, reject) => {
    const proc = spawn(binary, args, { stdio: ["pipe", "pipe", "pipe"] });
    let stdout = "",
      stderr = "",
      failure;
    const timer = setTimeout(() => {
      failure = new Error("Connection timed out");
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
        ? reject(failure)
        : code
          ? reject(new Error(stderr || `Process exited (${code})`))
          : resolve(stdout);
    });
    proc.stdin.on("error", () => {});
    proc.stdin.end(input);
  });
}
function validate(profile) {
  if (!profile || !/^[a-zA-Z0-9_][a-zA-Z0-9_.@-]*$/.test(profile.host || ""))
    throw new Error("Enter an SSH alias or user@hostname.");
  if (
    profile.port &&
    (!Number.isInteger(Number(profile.port)) ||
      Number(profile.port) < 1 ||
      Number(profile.port) > 65535)
  )
    throw new Error("Invalid SSH port");
  if (!/^(\/|~\/)[^\r\n\0:]+$/.test(profile.socket || ""))
    throw new Error("Enter an absolute remote socket path or ~/path.");
  return {
    id: /^[a-f0-9-]{36}$/.test(profile.id || "") ? profile.id : randomUUID(),
    name: String(profile.name || profile.host).slice(0, 80),
    host: profile.host,
    port: Number(profile.port) || undefined,
    socket: profile.socket,
  };
}
class Connections {
  constructor(dataDir) {
    this.file = path.join(dataDir, "connections.json");
    this.profiles = [];
    this.runtime = new Map();
    this.pending = new Map();
  }
  async init() {
    this.temp = await fs.mkdtemp("/tmp/sushiai-ssh-");
    try {
      this.profiles = JSON.parse(await fs.readFile(this.file, "utf8")).map(
        validate,
      );
    } catch (e) {
      if (e.code !== "ENOENT") throw e;
    }
  }
  list() {
    return this.profiles.map((p) => ({
      ...p,
      connected: this.runtime.has(p.id),
    }));
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
      "-o",
      "StrictHostKeyChecking=yes",
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
    await this.disconnect(`ssh:${p.id}`);
    this.profiles = [...this.profiles.filter((x) => x.id !== p.id), p];
    await fs.mkdir(path.dirname(this.file), { recursive: true });
    await fs.writeFile(this.file, JSON.stringify(this.profiles, null, 2), {
      mode: 0o600,
    });
    return p;
  }
  async delete(endpoint) {
    const p = this.get(endpoint);
    await this.disconnect(endpoint);
    this.profiles = this.profiles.filter((x) => x.id !== p.id);
    await fs.writeFile(this.file, JSON.stringify(this.profiles), {
      mode: 0o600,
    });
  }
  async inspect(endpoint, options) {
    const source = await fs.readFile(
      path.join(__dirname, "remote-files.py"),
      "utf8",
    );
    const p = endpoint?.startsWith("ssh:") ? this.get(endpoint) : null;
    const output = p
      ? await run(
          "/usr/bin/ssh",
          [...this.args(p), p.host, `python3 -c ${quote(source)}`],
          JSON.stringify(options),
        )
      : await run("/usr/bin/python3", ["-c", source], JSON.stringify(options));
    let envelope;
    try {
      envelope = JSON.parse(output);
    } catch {
      throw new Error(
        "Project reader returned invalid data. Check Python 3 and SSH shell startup output.",
      );
    }
    if (envelope.error) throw new Error(envelope.error);
    return envelope.result;
  }
  async socket(endpoint) {
    if (!endpoint.startsWith("ssh:")) {
      if (!path.isAbsolute(endpoint)) throw new Error("Invalid socket path");
      return endpoint;
    }
    const p = this.get(endpoint);
    if (this.runtime.has(p.id)) {
      const state = this.runtime.get(p.id);
      if (!state.proc.shared) return state.socket;
      try {
        await request(state.socket, "ping", {}, 1000);
        return state.socket;
      } catch {
        await this.disconnect(endpoint);
      }
    }
    if (this.pending.has(p.id)) return this.pending.get(p.id);
    const promise = this.connect(p).finally(() => this.pending.delete(p.id));
    this.pending.set(p.id, promise);
    return promise;
  }
  async forwardProcess(profile, specification) {
    let shared = false;
    try {
      await run(
        "/usr/bin/ssh",
        [...this.args(profile), "-O", "check", profile.host],
        "",
        3000,
      );
      shared = true;
    } catch {}
    if (shared) {
      await run("/usr/bin/ssh", [
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
          "/usr/bin/ssh",
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
    }
    return spawn(
      "/usr/bin/ssh",
      [
        ...this.args(profile),
        "-o",
        "ControlMaster=no",
        "-o",
        "ControlPath=none",
        "-N",
        "-o",
        "ExitOnForwardFailure=yes",
        "-L",
        specification,
        profile.host,
      ],
      { stdio: ["ignore", "ignore", "pipe"] },
    );
  }
  async connect(profile) {
    const home = await this.inspect(`ssh:${profile.id}`, {
      operation: "home",
      socket: profile.socket,
    });
    if (!/^\/[^\r\n\0:]+$/.test(home.socket))
      throw new Error("Invalid resolved socket path");
    const socketPath = path.join(this.temp, profile.id.slice(0, 8) + ".sock");
    await fs.rm(socketPath, { force: true });
    const proc = await this.forwardProcess(
      profile,
      `${socketPath}:${home.socket}`,
    );
    let error = "",
      exited = false;
    proc.stderr.on("data", (d) => {
      error = (error + d).slice(-3000);
    });
    proc.on("error", (e) => {
      error = e.message;
      exited = true;
    });
    const state = {
      proc,
      socket: socketPath,
      forwards: new Map(),
      home: home.home,
    };
    proc.on("exit", () => {
      exited = true;
      if (this.runtime.get(profile.id) === state) {
        for (const child of state.forwards.values()) child.proc.kill();
        this.runtime.delete(profile.id);
      }
    });
    for (let attempt = 0; attempt < 40; attempt++) {
      if (exited) break;
      try {
        await fs.stat(socketPath);
        await request(socketPath, "ping", {}, 1000);
        this.runtime.set(profile.id, state);
        return socketPath;
      } catch {}
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
    await proc.kill();
    await fs.rm(socketPath, { force: true });
    throw new Error(
      error ||
        "SSH connected, but Herdr is not running at this socket. Start Herdr on the host or choose another session socket.",
    );
  }
  async forward(endpoint, port) {
    if (!Number.isInteger(port) || port < 1 || port > 65535)
      throw new Error("Invalid port");
    await this.socket(endpoint);
    const p = this.get(endpoint),
      state = this.runtime.get(p.id);
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
    const id = endpoint.replace(/^ssh:/, "");
    if (this.pending.has(id)) await this.pending.get(id).catch(() => {});
    const state = this.runtime.get(id);
    if (state) {
      for (const forward of state.forwards.values()) await forward.proc.kill();
      await state.proc.kill();
      this.runtime.delete(id);
    }
  }
  async close() {
    await Promise.allSettled([...this.pending.values()]);
    for (const id of this.runtime.keys()) await this.disconnect(`ssh:${id}`);
    if (this.temp) await fs.rm(this.temp, { recursive: true, force: true });
  }
}
module.exports = { Connections, run, quote, validate };
