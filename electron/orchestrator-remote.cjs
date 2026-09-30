// An orchd daemon on an SSH host, reached over the Connections machinery: the
// app installs/updates `~/.sushiai/bin/orchd` there, starts it detached (so it
// outlives the app and the laptop's sleep), forwards its unix socket to a local
// temp socket and reads its control token over ssh. No port is opened, and
// the app never stops a remote daemon: `close()` only drops the forward.
const fs = require("node:fs/promises");
const path = require("node:path");
const { spawn } = require("node:child_process");
const { createHash } = require("node:crypto");

const RUSTUP_COMMAND =
  "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y";
const REMOTE_DATA = "$HOME/.sushiai/orchestrator";
const REMOTE_BIN = "$HOME/.sushiai/bin";
// A non-interactive ssh shell often lacks the user's tool directories.
const REMOTE_PATH =
  'export PATH="$HOME/.local/bin:$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:$PATH"';
const BUILD_TIMEOUT_MS = 20 * 60 * 1000;
const UPLOAD_TIMEOUT_MS = 5 * 60 * 1000;
const RETRY_AFTER_MS = 30 * 1000;

// The setup errors the renderer reads the platform and the rustup command
// back out of (`hostPlatform`/`rustupCommand` in src/orchestrator/hosts.ts);
// tests/orchestrator-hosts.test.cjs parses these very strings.
function needsRustMessage(name, platform) {
  return `Rust is not installed on ${name} (${platform || "unknown platform"}), and orchd has to be built there. Install it on the host with: ${RUSTUP_COMMAND} - then connect again.`;
}

function noSourceMessage(name, platform, wanted) {
  return `This build of sushiAI has no orchd source to build on ${name} (${platform || "unknown platform"}); it can only upload its own binary to a ${wanted} host.`;
}

/** One `key=value` per line, as the probe scripts below print them. */
function parseProbe(output) {
  const values = {};
  for (const line of String(output).split("\n")) {
    const match = /^([a-z_]+)=(.*)$/.exec(line.trim());
    if (match) values[match[1]] = match[2];
  }
  return values;
}

const INFO_SCRIPT = `${REMOTE_PATH}
echo "home=$HOME"
echo "platform=$(uname -sm)"
[ -x "${REMOTE_BIN}/orchd" ] && echo "installed=1"
[ -f "${REMOTE_BIN}/orchd.hash" ] && echo "hash=$(cat "${REMOTE_BIN}/orchd.hash")"
command -v cargo >/dev/null 2>&1 && echo "cargo=1"
exit 0`;

// `claude auth status` / `codex login status` exit non-zero when logged out.
// Bounded with `timeout` when the host has it: a CLI waiting on a prompt must
// not hang the probe.
const PREFLIGHT_SCRIPT = `${REMOTE_PATH}
t() { if command -v timeout >/dev/null 2>&1; then timeout 10 "$@"; else "$@"; fi; }
command -v git >/dev/null 2>&1 && echo "git=1"
if command -v claude >/dev/null 2>&1; then echo "claude=1"; t claude auth status >/dev/null 2>&1 && echo "claude_login=1"; fi
if command -v codex >/dev/null 2>&1; then echo "codex=1"; t codex login status >/dev/null 2>&1 && echo "codex_login=1"; fi
exit 0`;

/** What a host offers orchd's routes: `git` plus each harness CLI's
 * presence and login. */
function parsePreflight(output, now = Date.now()) {
  const values = parseProbe(output);
  const harness = (name) => ({
    installed: values[name] === "1",
    loggedIn: values[`${name}_login`] === "1",
  });
  return {
    git: values.git === "1",
    claude: harness("claude"),
    codex: harness("codex"),
    checkedAt: now,
  };
}

/** What must happen for the host's binary to match the app's. */
function installPlan(info, want) {
  if (info.installed === "1" && info.hash === want.hash) return "none";
  if (want.binary && info.platform === want.platform) return "upload";
  return "build";
}

function startScript() {
  const run = `"${REMOTE_BIN}/orchd" serve --data "$d" --socket "$d/orchd.sock" >>"$d/orchd.log" 2>&1 </dev/null &`;
  return `${REMOTE_PATH}
d="${REMOTE_DATA}"
mkdir -p "$d"
if [ -f "$d/orchd.pid" ] && kill -0 "$(cat "$d/orchd.pid")" 2>/dev/null; then echo "running=1"; exit 0; fi
if command -v setsid >/dev/null 2>&1; then nohup setsid ${run}
else nohup ${run}
fi
echo "started=1"
exit 0`;
}

function uploadScript(hash) {
  return `set -e
b="${REMOTE_BIN}"
mkdir -p "$b"
cat > "$b/orchd.new"
chmod 755 "$b/orchd.new"
mv "$b/orchd.new" "$b/orchd"
printf %s ${hash} > "$b/orchd.hash"`;
}

function buildScript(hash) {
  return `set -e
${REMOTE_PATH}
s="$HOME/.sushiai/src"
b="${REMOTE_BIN}"
rm -rf "$s"
mkdir -p "$s" "$b"
tar -xf - -C "$s"
cd "$s/orchd"
cargo build --release
cp target/release/orchd "$b/orchd.new"
chmod 755 "$b/orchd.new"
mv "$b/orchd.new" "$b/orchd"
printf %s ${hash} > "$b/orchd.hash"`;
}

async function sourceFiles(directory, base = directory) {
  const entries = await fs.readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries.sort((a, b) => (a.name < b.name ? -1 : 1))) {
    if (entry.name === "target" || entry.name === ".git") continue;
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...(await sourceFiles(full, base)));
    else if (entry.isFile()) files.push(path.relative(base, full));
  }
  return files;
}

function uname() {
  return new Promise((resolve, reject) => {
    const child = spawn("uname", ["-sm"], {
      stdio: ["ignore", "pipe", "ignore"],
    });
    let out = "";
    child.stdout.on("data", (chunk) => (out += chunk));
    child.on("error", reject);
    child.on("close", () => resolve(out.trim()));
  });
}

/** What the app can put on a host: its own orchd binary (same platform only)
 * and the orchd source (any platform, built there). The version key is a hash
 * of `orchd/` (working tree, `target/` excluded) - or of the binary when the
 * source is not shipped, as in a packaged app. */
function localArtifacts({ root, binary }) {
  const sourceDir = path.join(root, "orchd");
  let memo = null;
  const sha = async () => {
    const hash = createHash("sha256");
    let hasSource = false;
    try {
      await fs.access(path.join(sourceDir, "Cargo.toml"));
      hasSource = true;
    } catch {
      // No source next to the app (packaged): key on the binary instead.
    }
    if (hasSource) {
      for (const file of await sourceFiles(sourceDir)) {
        hash.update(file + "\0");
        hash.update(await fs.readFile(path.join(sourceDir, file)));
      }
    } else {
      hash.update(await fs.readFile(binary));
    }
    return { hash: hash.digest("hex").slice(0, 32), hasSource };
  };
  return {
    async describe() {
      if (memo) return memo;
      let binaryExists = false;
      try {
        await fs.access(binary);
        binaryExists = true;
      } catch {
        // Not built: only a source build on the host is possible.
      }
      const { hash, hasSource } = await sha();
      memo = {
        hash,
        binary: binaryExists ? binary : null,
        source: hasSource,
        platform: await uname(),
      };
      return memo;
    },
    readBinary: () => fs.readFile(binary),
    archive() {
      return new Promise((resolve, reject) => {
        const child = spawn(
          "tar",
          ["-cf", "-", "--exclude", "target", "-C", root, "orchd"],
          {
            stdio: ["ignore", "pipe", "pipe"],
            env: { ...process.env, COPYFILE_DISABLE: "1" },
          },
        );
        const chunks = [];
        let error = "";
        child.stdout.on("data", (chunk) => chunks.push(chunk));
        child.stderr.on("data", (chunk) => (error += chunk));
        child.on("error", reject);
        child.on("close", (code) =>
          code
            ? reject(new Error(error || `tar exited (${code})`))
            : resolve(Buffer.concat(chunks)),
        );
      });
    },
  };
}

/** One host's daemon: `ensure()` brings it to a reachable, authenticated
 * state and returns `{ socketPath, token }`. Single-flight. */
class RemoteOrchd {
  constructor({
    connections,
    endpoint,
    artifacts,
    request,
    onChange,
    spawnRetries = 50,
    spawnIntervalMs = 200,
  }) {
    this.connections = connections;
    this.endpoint = endpoint;
    this.artifacts = artifacts;
    this.request = request;
    this.onChange = onChange;
    this.spawnRetries = spawnRetries;
    this.spawnIntervalMs = spawnIntervalMs;
    this.state = "idle";
    this.detail = "";
    this.preflight = null;
    // What the probe found: `uname -sm`, and whether any orchd is installed.
    this.platform = "";
    this.orchdInstalled = null;
    this.conn = null;
    this.inflight = null;
    // The last failed setup: requests fail fast with it until
    // RETRY_AFTER_MS pass or the owner retries, so a host that is down is not
    // re-provisioned over SSH on every request.
    this.failure = null;
    this.closed = false;
  }

  /** Forgets the last failed setup: the owner's Try again or recheck. */
  forget() {
    this.failure = null;
  }

  #set(state, detail = "") {
    this.state = state;
    this.detail = detail;
    this.onChange?.();
  }

  get name() {
    return this.connections.get(this.endpoint).name;
  }

  async ensure() {
    if (this.closed) throw new Error("orchestrator closed");
    if (this.conn) {
      try {
        await this.request(this.conn.socketPath, "ping", {}, null, 2000);
        return this.conn;
      } catch {
        this.#drop();
      }
    }
    if (
      !this.inflight &&
      this.failure &&
      Date.now() - this.failure.at < RETRY_AFTER_MS
    )
      throw this.failure.error;
    if (!this.inflight)
      this.inflight = this.#provision()
        .then((conn) => {
          this.failure = null;
          this.#set("ready");
          return conn;
        })
        .catch((error) => {
          this.failure = { at: Date.now(), error };
          this.#set("error", error.message);
          throw error;
        })
        .finally(() => {
          this.inflight = null;
        });
    return this.inflight;
  }

  #drop() {
    const conn = this.conn;
    this.conn = null;
    conn?.proc.kill();
  }

  async #provision() {
    this.#set("connecting");
    const info = parseProbe(
      await this.connections.exec(this.endpoint, INFO_SCRIPT),
    );
    if (!info.home || !info.home.startsWith("/"))
      throw new Error("Could not read the home directory on the host.");
    this.platform = info.platform || "";
    this.orchdInstalled = info.installed === "1";
    this.onChange?.();
    // What the host offers is known before orchd is, so the setup card can
    // show it while orchd builds (or fails to).
    void this.refreshPreflight().catch(() => {});
    const want = await this.artifacts.describe();
    const plan = installPlan(info, want);
    if (plan !== "none") {
      // A running old daemon is asked to shut down first (its tasks resume
      // under the new one); the connection is best-effort.
      await this.#shutdownRunning(info.home);
      await this.#install(plan, info, want);
    }
    this.#set("starting");
    await this.connections.exec(this.endpoint, startScript());
    const conn = await this.#connect(info.home);
    this.conn = conn;
    return conn;
  }

  async #shutdownRunning(home) {
    try {
      const conn = await this.#connect(home, { attempts: 1 });
      try {
        await this.request(
          conn.socketPath,
          "shutdown",
          {},
          conn.token,
          5000,
        ).catch(() => {});
      } finally {
        conn.proc.kill();
      }
      await new Promise((resolve) => setTimeout(resolve, 300));
    } catch {
      // Nothing running: nothing to stop.
    }
  }

  async #install(plan, info, want) {
    const name = this.name;
    if (plan === "upload") {
      this.#set("installing", `Uploading orchd to ${name}`);
      await this.connections.exec(this.endpoint, uploadScript(want.hash), {
        input: await this.artifacts.readBinary(),
        timeout: UPLOAD_TIMEOUT_MS,
      });
      return;
    }
    if (info.cargo !== "1")
      throw new Error(needsRustMessage(name, info.platform));
    if (!want.source)
      throw new Error(noSourceMessage(name, info.platform, want.platform));
    this.#set("building", `Building orchd on ${name} (a few minutes)`);
    await this.connections.exec(this.endpoint, buildScript(want.hash), {
      input: await this.artifacts.archive(),
      timeout: BUILD_TIMEOUT_MS,
    });
  }

  async #connect(home, { attempts = this.spawnRetries } = {}) {
    const dataDir = `${home}/.sushiai/orchestrator`;
    const socketPath = path.join(
      this.connections.temp,
      `orchd-${this.endpoint.replace(/^ssh:/, "").slice(0, 8)}.sock`,
    );
    await fs.rm(socketPath, { force: true });
    const proc = await this.connections.forwardSocket(
      this.endpoint,
      socketPath,
      `${dataDir}/orchd.sock`,
    );
    let exited = false;
    proc.on("error", () => (exited = true));
    proc.on("exit", () => {
      exited = true;
      if (this.conn?.proc === proc) {
        this.conn = null;
        if (!this.closed) this.#set("error", "The SSH connection dropped.");
      }
    });
    try {
      let token = null;
      for (let attempt = 0; attempt < attempts && !exited; attempt++) {
        try {
          token ??= (
            await this.connections.exec(
              this.endpoint,
              `cat "${dataDir}/control.token"`,
              { timeout: 8000 },
            )
          ).trim();
          if (!token) throw new Error("no token yet");
          await this.request(socketPath, "ping", {}, token, 2000);
          return { socketPath, token, proc };
        } catch {
          token = null;
          await new Promise((resolve) =>
            setTimeout(resolve, this.spawnIntervalMs),
          );
        }
      }
    } catch {
      // Falls through to the failure below.
    }
    proc.kill();
    throw new Error(
      `The orchestrator on ${this.name} did not answer. Check ~/.sushiai/orchestrator/orchd.log on the host.`,
    );
  }

  async refreshPreflight() {
    this.preflight = parsePreflight(
      await this.connections.exec(this.endpoint, PREFLIGHT_SCRIPT, {
        timeout: 40000,
      }),
    );
    this.onChange?.();
    return this.preflight;
  }

  /** Drops the forward only - the daemon keeps running on the host. */
  close() {
    this.closed = true;
    this.#drop();
  }

  reopen() {
    this.closed = false;
  }
}

module.exports = {
  RemoteOrchd,
  localArtifacts,
  parseProbe,
  parsePreflight,
  installPlan,
  startScript,
  uploadScript,
  buildScript,
  RUSTUP_COMMAND,
  needsRustMessage,
  noSourceMessage,
};
