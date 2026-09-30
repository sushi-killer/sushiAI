// The orchestrator daemon (`orchd/`, Rust): this module finds/spawns it,
// speaks its NDJSON protocol, and relays its `subscribe` stream to the
// renderer. Nothing starts at launch: a daemon that is already running is
// attached to (never spawned), and the first real request spawns one. Closing
// the window leaves it running; quitting the app shuts down the local daemon
// this app spawned (a test launch, `stopDaemonOnQuit`, also stops one it
// merely found). A remote host's daemon is only disconnected, never stopped.
// orchd also exits on its own when its data dir is deleted.
const net = require("node:net");
const path = require("node:path");
const os = require("node:os");
const fs = require("node:fs/promises");
const { existsSync } = require("node:fs");
const { spawn } = require("node:child_process");
const TEST_MCP_HOME = require("node:path").join(
  __dirname,
  "test-fixtures",
  "mcp-home",
);
const { createHash, randomUUID } = require("node:crypto");
const { RemoteOrchd, localArtifacts } = require("./orchestrator-remote.cjs");

const LOCAL_HOST = "local";
const OFF_MESSAGE = "The orchestrator is off.";

/** What every orchestrator request rejects with while the extension is off,
 * so nothing spawns, connects or provisions a host. */
function offError() {
  const error = new Error(OFF_MESSAGE);
  error.code = "ORCHESTRATOR_OFF";
  return error;
}

const ORCHESTRATOR_MANIFEST = {
  id: "builtin.orchestrator",
  name: "Orchestrator",
  version: "1.0.0",
  apiVersion: 1,
  source: { kind: "builtin" },
  scope: "app",
  description: "Carries tasks to a verified, committed done.",
  contributions: { surfaces: [], navigation: [], actions: [], commands: [] },
};

// The renderer only ever reaches these; `hook.stop` (the Claude Stop hook)
// and `shutdown` are daemon-internal / CLI-only, never IPC-reachable.
const ALLOWED_METHODS = new Set([
  "ping",
  "settings.get",
  "settings.set",
  "settings.defaults",
  "task.list",
  "task.get",
  "task.create",
  "task.start",
  "task.land",
  "task.pr",
  "task.stop",
  "task.answer",
  "task.overturn",
  "task.leadTouch",
  "task.delete",
  "task.archive",
  "task.unarchive",
  "task.timeline",
  "task.evidence",
  "task.openDeliverable",
  "task.report",
  "task.backlog",
  "failures.catalogue",
  "costs.summary",
  "chat.get",
  "chat.send",
  "chat.edit",
  "chat.cancel",
  "chat.list",
  "chat.new",
  "chat.switch",
  "chat.clear",
  "chat.createProposal",
  "chat.tools",
  "chat.toolServers",
  "chat.actionSend",
  "chat.actionDecline",
  "message.list",
  "message.send",
  "evolution.run",
  "evolution.list",
  "evolution.approve",
  "evolution.reject",
  "evolution.adopt",
  "repo.notes.list",
  "repo.notes.add",
  "repo.notes.remove",
]);

// Results that are one task or a list of them: a remote host's are tagged
// with the host so the renderer knows where a task lives.
const TASK_RESULT_METHODS = new Set([
  "task.list",
  "task.get",
  "task.create",
  "task.start",
  "task.land",
  "task.pr",
  "task.stop",
  "task.answer",
  "task.archive",
  "task.unarchive",
  "task.overturn",
  "task.leadTouch",
  "task.backlog",
]);

function tagTasks(result, host) {
  const isTask = (value) =>
    value && typeof value === "object" && "id" in value && "status" in value;
  if (Array.isArray(result))
    return result.map((task) => (isTask(task) ? { ...task, host } : task));
  return isTask(result) ? { ...result, host } : result;
}

/** A relayed event from a remote host, its task (if any) tagged. */
function tagEvent(message, host) {
  if (!message || typeof message !== "object") return message;
  const tagged = { ...message, host };
  if (message.task && typeof message.task === "object")
    tagged.task = { ...message.task, host };
  return tagged;
}

// The stages that can ask the owner (`AskedBy` in src/orchestrator/types.ts);
// anything else a daemon sends is dropped from the notice.
const ASKED_BY = new Set([
  "brief",
  "plan",
  "implement",
  "verify",
  "review",
  "advisor",
  "land",
]);

const NOT_BUILT =
  "The orchestrator daemon is not built. Run npm run build:orchd.";
const NOT_INSTALLED =
  "The orchestrator is missing from this installation — reinstall sushiAI.";

/** A packaged app has no toolchain to build with, so a missing daemon means a
 * broken install; only a source checkout is told to build it. */
function notBuiltMessage(packaged) {
  return packaged ? NOT_INSTALLED : NOT_BUILT;
}
const FAILED_TO_START = "The orchestrator daemon failed to start.";

function orchdBinaryPath({ root, resourcesPath, packaged }) {
  return packaged
    ? path.join(resourcesPath, "orchd")
    : path.join(root, "orchd", "target", "release", "orchd");
}

// A unix socket path is capped around 100 bytes on macOS; userData can nest
// deep enough (a long account name, iCloud Drive, ...) to blow past that, so
// a too-long path falls back to a short, stable name under $TMPDIR instead.
function socketPathFor(dataDir, tmpDir = os.tmpdir()) {
  const candidate = path.join(dataDir, "orchd.sock");
  if (Buffer.byteLength(candidate, "utf8") <= 100) return candidate;
  const hash = createHash("sha256").update(dataDir).digest("hex").slice(0, 8);
  return path.join(tmpDir, `sushi-orchd-${hash}.sock`);
}

// A small NDJSON-RPC client dedicated to orchd's own control-auth envelope
// (`herdr.cjs`'s `request()` has no `auth` field and Herdr's own protocol
// must not gain one just for this). Every request but `ping` carries the
// current control token alongside id/method/params.
function orchdRequest(socketPath, method, params, token, timeout = 5000) {
  return new Promise((resolve, reject) => {
    const id = randomUUID();
    const socket = net.createConnection(socketPath);
    let buffer = "",
      settled = false;
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      socket.destroy();
      error ? reject(error) : resolve(value);
    };
    socket.setEncoding("utf8");
    socket.setTimeout(timeout, () =>
      finish(new Error("The orchestrator daemon did not respond.")),
    );
    socket.on("error", (error) => finish(error));
    socket.on("close", () => {
      if (!settled)
        finish(
          new Error("The orchestrator daemon disconnected before responding."),
        );
    });
    socket.on("connect", () => {
      const envelope = { id, method, params };
      if (method !== "ping" && token) envelope.auth = token;
      socket.write(JSON.stringify(envelope) + "\n");
    });
    socket.on("data", (chunk) => {
      buffer += chunk;
      if (buffer.length > 16 * 1024 * 1024)
        return finish(new Error("Orchestrator response exceeds 16 MB."));
      let boundary;
      while ((boundary = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 1);
        if (!line.trim()) continue;
        let message;
        try {
          message = JSON.parse(line);
        } catch {
          return finish(
            new Error("Invalid JSON from the orchestrator daemon."),
          );
        }
        if (message.id !== id) continue;
        if (message.error) return finish(new Error(message.error.message));
        finish(null, message.result);
      }
    });
  });
}

/** How long a rebuilt binary waits for running attempts to finish before
 * it replaces the daemon anyway. */
const STALE_DEFER_MS = 30 * 60 * 1000;

/** Whether a running daemon's own binary has since been rebuilt: the binary
 * on disk is more than a second newer than the one it loaded. Running
 * attempts defer the replacement for up to `STALE_DEFER_MS` (`staleForMs` =
 * how long the rebuild has been seen): a replacement interrupts them and the
 * next daemon restarts each as a new attempt, so every landing's `afterLand`
 * rebuild would otherwise cut every live task short. An in-flight chat reply
 * (`chatTurns`) always blocks: chat has no resume path. */
function isStalePing(ping, actualBinaryMtimeMs, staleForMs = 0) {
  if (!ping?.binaryMtimeMs || ping.chatTurns > 0) return false;
  if (actualBinaryMtimeMs <= ping.binaryMtimeMs + 1000) return false;
  return !(ping.running > 0) || staleForMs >= STALE_DEFER_MS;
}

// Task statuses (orchd/src/model.rs TaskStatus) that need no daemon; every
// other one (drafting, queued, running, waiting, landing) does. An archived
// task never needs one.
const FINISHED_STATUSES = new Set(["done", "failed", "stopped"]);

/** Polls `killFn` (default: a zero-signal `kill`, which throws once the pid
 * is gone) until the process exits or `timeoutMs` passes. Best-effort: never
 * rejects, since a stuck old process just means the next call retries. */
async function waitForExit(
  pid,
  {
    killFn = (p) => process.kill(p, 0),
    timeoutMs = 5000,
    intervalMs = 100,
  } = {},
) {
  if (!pid) return;
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      killFn(pid);
    } catch {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }
}

const FAILURE_LABELS = {
  no_deliverable: "no deliverable",
  stall: "stalled",
  loop: "looping",
  budget: "over budget",
  verify: "verify failed",
  heldout: "held-out check failed",
  review: "review failed",
  protected: "protected path touched",
  blocked: "blocked",
  error: "error",
};

/** A stop the engine chose rather than the owner (mirrors
 * `stoppedByEngine` in src/orchestrator/ownerAttention.ts): the owner's stop
 * leaves an "Owner:" decision or an interrupted attempt. */
function stoppedByEngine(task) {
  if (task.status !== "stopped") return false;
  const decisions = Array.isArray(task.decisions) ? task.decisions : [];
  const last = String(decisions[decisions.length - 1] ?? "");
  if (last.startsWith("Owner:")) return false;
  if (last.startsWith("Orchestrator:")) return true;
  const attempts = Array.isArray(task.attempts) ? task.attempts : [];
  const attempt = attempts[attempts.length - 1];
  return !!attempt && attempt.status !== "interrupted" && !!attempt.failure;
}

function formatCost(costUsd) {
  return `$${(costUsd || 0).toFixed(2)}`;
}

/** How long after its `reportAt` a report still counts as fresh. */
const REPORT_NOTICE_MS = 5 * 60 * 1000;

const NOTICE_MAX_OPTIONS = 4;
const NOTICE_OPTION_CHARS = 400;
const NOTICE_INPUT_BODY_CHARS = 2000;

/** The notice for a task that needs input, finished or failed - trimmed to
 * the caps a native notification can carry. Pure: the caller decides whether
 * the task is worth one. */
function orchestratorNotice(task) {
  const cap = (value, max) => String(value ?? "").slice(0, max);
  let title = cap(task.title || "Orchestrator", 120);
  let kind = "input";
  let body = cap(
    task.question?.text || "Needs your input.",
    NOTICE_INPUT_BODY_CHARS,
  );
  let focus = "question";
  let canLand = false;
  if (task.status === "done") {
    kind = "done";
    focus = "report";
    // `reportAt` is set once; an old report re-broadcast after a restart is
    // not news, so only a fresh one is announced as a finished feature.
    const fresh = Date.now() - Number(task.reportAt || 0) < REPORT_NOTICE_MS;
    if (task.report && fresh)
      title = cap(`Feature done: ${task.title || "Orchestrator"}`, 120);
    const where = task.landedSha
      ? `landed ${String(task.landedSha).slice(0, 8)} on ${task.baseRef || "its base"}`
      : `on ${task.branch || "its branch"}, not landed`;
    body = cap(`${formatCost(task.costUsd)} · ${where}`, 300);
    if (!task.landedSha && !task.parent && task.baseRef && !task.archived)
      canLand = true;
  } else if (task.status === "failed") {
    kind = "failed";
    focus = "summary";
    const attempts = Array.isArray(task.attempts) ? task.attempts : [];
    const last = attempts[attempts.length - 1];
    const failure = last?.failure?.kind;
    const label = FAILURE_LABELS[failure] || failure || "failed";
    body = cap(`${label} · ${formatCost(task.costUsd)}`, 300);
  } else if (task.status === "landing") {
    kind = "landing";
    focus = "summary";
    body = "Waiting for a clean checkout to land.";
  } else if (task.status === "stopped") {
    kind = "stopped";
    focus = "summary";
    body = "Stopped on its own - needs a look.";
  }
  const notice = {
    taskId: cap(task.id, 200),
    repo: cap(task.repo, 1000),
    kind,
    title,
    body,
    focus,
  };
  if (canLand) notice.canLand = true;
  if (typeof task.host === "string" && task.host && task.host !== LOCAL_HOST)
    notice.host = cap(task.host, 200);
  const at = Number(task.question?.askedAt) || Number(task.updatedAt) || 0;
  if (at > 0) notice.at = at;
  if (kind === "input" && ASKED_BY.has(task.question?.askedBy))
    notice.askedBy = task.question.askedBy;
  const repoName = String(task.repo ?? "")
    .split(/[\\/]+/)
    .filter(Boolean)
    .pop();
  if (repoName) notice.repoName = cap(repoName, 200);
  if (kind === "done" || kind === "failed") {
    notice.costUsd = Number(task.costUsd) || 0;
    const attempts = Array.isArray(task.attempts) ? task.attempts : [];
    const verdict = [...attempts]
      .reverse()
      .find((attempt) => attempt?.review?.verdict)?.review.verdict;
    if (verdict === "PASS" || verdict === "FAIL") notice.verdict = verdict;
    if (kind === "done") notice.landed = !!task.landedSha;
  }
  if (kind === "input") {
    const options = (
      Array.isArray(task.question?.options) ? task.question.options : []
    )
      .filter((option) => typeof option === "string" && option.trim())
      .slice(0, NOTICE_MAX_OPTIONS)
      .map((option) => cap(option, NOTICE_OPTION_CHARS));
    if (options.length) notice.options = options;
  }
  return notice;
}

class OrchestratorService {
  constructor({
    dataDir,
    root,
    resourcesPath,
    packaged,
    send,
    notify,
    onTask,
    getClaudeMcp,
    getModelProviders,
    getProjects,
    spawnRetries = 50,
    spawnIntervalMs = 100,
    stopDaemonOnQuit = false,
    // orchd stops its agents on SIGTERM (5 s escalation) before it exits.
    quitTimeoutMs = 7000,
    host = LOCAL_HOST,
    remote = null,
  }) {
    this.host = host;
    // A remote service reaches its daemon through `remote` (the ssh
    // forward and token); it never spawns, restarts or stops one itself.
    this.remote = remote;
    this.dataDir = dataDir;
    this.socketPath = dataDir ? socketPathFor(dataDir) : null;
    this.packaged = Boolean(packaged);
    this.binary = remote
      ? null
      : orchdBinaryPath({ root, resourcesPath, packaged });
    this.send = remote
      ? (channel, message) => send(channel, tagEvent(message, host))
      : send;
    this.notify = notify;
    this.onTask = onTask;
    this.getClaudeMcp = getClaudeMcp;
    this.getModelProviders = getModelProviders;
    this.getProjects = getProjects;
    this.spawnRetries = spawnRetries;
    this.spawnIntervalMs = spawnIntervalMs;
    this.stopDaemonOnQuit = stopDaemonOnQuit;
    this.quitTimeoutMs = quitTimeoutMs;
    this.token = null;
    this.subscribeSocket = null;
    this.backoff = 500;
    this.closed = false;
    // Bumped by every connect() and close(), so a subscribe loop that was
    // mid-await when the service was closed and reopened cannot keep running
    // beside the new one.
    this.epoch = 0;
    this.retryTimer = null;
    // Whether a subscribe loop is running, whether a real request has been
    // made (until then the loop only attaches, it never spawns), and whether
    // this app spawned the daemon it talks to (so quit() may stop it).
    this.looping = false;
    this.used = false;
    this.spawned = false;
    this.starting = null;
    this.running = null;
    // Keys of notices already raised (see #notifyTransition): a task
    // re-entering a state with the same question never re-notifies.
    this.notified = new Set();
    // Last status seen per task: failed/landing/stopped notify only on a
    // real change into that status, not when first seen already there.
    this.lastStatus = new Map();
  }

  async #refreshToken() {
    if (this.remote) return;
    try {
      this.token = (
        await fs.readFile(path.join(this.dataDir, "control.token"), "utf8")
      ).trim();
    } catch {
      this.token = null;
    }
  }

  async #isStale(ping) {
    if (!ping?.binaryMtimeMs || ping.chatTurns > 0) return false;
    try {
      const { mtimeMs } = await fs.stat(this.binary);
      if (mtimeMs <= ping.binaryMtimeMs + 1000) {
        this.staleSeenAt = null;
        return false;
      }
      this.staleSeenAt ??= Date.now();
      return isStalePing(ping, mtimeMs, Date.now() - this.staleSeenAt);
    } catch {
      return false;
    }
  }

  // Tracks the whole in-flight call so quit() can wait for it, not only for
  // the spawn at its end.
  async #ensureRunning() {
    const run = this.#ensureRunningInner();
    this.running = run;
    try {
      return await run;
    } finally {
      if (this.running === run) this.running = null;
    }
  }

  async #ensureRunningInner() {
    if (this.closed) throw new Error("orchestrator closed");
    if (this.remote) {
      const conn = await this.remote.ensure();
      this.socketPath = conn.socketPath;
      this.token = conn.token;
      return { remote: true };
    }
    if (!existsSync(this.binary))
      throw new Error(notBuiltMessage(this.packaged));
    await this.#refreshToken();
    try {
      const ping = await orchdRequest(
        this.socketPath,
        "ping",
        {},
        this.token,
        2000,
      );
      if (!(await this.#isStale(ping))) return ping;
      // A rebuilt binary replaces the daemon once no attempt runs (or the
      // deferral ran out); wait for the old process to actually exit before
      // spawning the new one on the same socket path.
      this.staleSeenAt = null;
      await orchdRequest(
        this.socketPath,
        "shutdown",
        {},
        this.token,
        5000,
      ).catch(() => {});
      await waitForExit(ping.pid);
    } catch {
      // Fall through to spawn: one in-flight spawn per service, so a burst of
      // calls before the daemon is up doesn't race several children.
    }
    if (this.closed) throw new Error("orchestrator closed");
    if (!this.starting) this.starting = this.#spawnAndWait();
    try {
      return await this.starting;
    } finally {
      this.starting = null;
    }
  }

  async #spawnAndWait() {
    if (this.closed) throw new Error("orchestrator closed");
    await fs.mkdir(this.dataDir, { recursive: true });
    if (this.closed) throw new Error("orchestrator closed");
    const child = spawn(
      this.binary,
      // The owner's own daemon installs the sushiai-orchestrator skill; a
      // test launch never writes into the owner's home.
      [
        "--data",
        this.dataDir,
        "--socket",
        this.socketPath,
        ...(this.stopDaemonOnQuit ? [] : ["--install-skill"]),
      ],
      {
        detached: true,
        stdio: "ignore",
        // A test launch reads MCP servers from a fixture, never the owner's
        // own ~/.claude.json, so screenshots show no real server names.
        env: this.stopDaemonOnQuit
          ? { ...process.env, SUSHIAI_MCP_HOME: TEST_MCP_HOME }
          : process.env,
      },
    );
    // A binary that vanished (a removed install or data dir) is reported by
    // the ping loop below as a failed start, never as an uncaught error that
    // would take down the main process.
    child.once("error", () => {});
    // Detached and unref'd on purpose: this process must outlive the app.
    child.unref();
    this.spawned = true;
    for (let attempt = 0; attempt < this.spawnRetries; attempt++) {
      if (this.closed) throw new Error("orchestrator closed");
      await this.#refreshToken();
      try {
        return await orchdRequest(
          this.socketPath,
          "ping",
          {},
          this.token,
          2000,
        );
      } catch {
        await new Promise((resolve) =>
          setTimeout(resolve, this.spawnIntervalMs),
        );
      }
    }
    throw new Error(FAILED_TO_START);
  }

  async #withMcp(params) {
    // The project's .mcp.json lives on this machine; a remote repo path has none here.
    if (this.remote || typeof params?.repo !== "string") return params;
    if (this.getProjects) {
      try {
        const project = await this.getProjects().resolveDirectory(params.repo);
        if (project) {
          params = { ...params, projectId: project.id };
          if (Object.keys(project.mcp || {}).length) {
            const current = params.mcp?.mcpServers || params.mcp || {};
            params.mcp = { mcpServers: { ...project.mcp, ...current } };
          }
        }
      } catch {}
    }
    if (!this.getClaudeMcp) return params;
    try {
      const mcp = await this.getClaudeMcp().launchConfig(params.repo);
      const project = params.mcp?.mcpServers || params.mcp || {};
      const local = mcp?.mcpServers || mcp || {};
      return {
        ...params,
        mcp: { mcpServers: { ...local, ...project } },
      };
    } catch {
      // No .mcp.json / no readable project: the task still starts, just
      // without MCP servers wired into the Claude run.
      return params;
    }
  }

  /** Full replace, always pushed (connect + after every `settings.set`):
   * `profiles` is every route's resolved model-profile env + key. Nothing
   * is staged to disk - `resolveEnv` hands the env map and key back in memory. */
  async #pushSecrets() {
    // API keys never leave this machine: a remote daemon uses the harness
    // logins already on its host.
    if (this.remote || !this.getModelProviders) return;
    let settings;
    try {
      settings = await orchdRequest(
        this.socketPath,
        "settings.get",
        {},
        this.token,
        5000,
      );
    } catch {
      return;
    }
    const providers = this.getModelProviders();
    const profileIds = [
      ...new Set(
        (settings?.routes || [])
          .map((route) => route.profileId)
          .filter((id) => typeof id === "string" && id),
      ),
    ];
    const profiles = {};
    const accounts = {};
    for (const id of profileIds) {
      try {
        const { settings: env, key } = await providers.resolveEnv(id);
        profiles[id] = { env, key };
      } catch {
        // Missing key / deleted profile: omitted, so that route falls back
        // to the tier's plain route, per the profile-fallback contract.
      }
    }
    for (const id of new Set(
      (settings?.routes || [])
        .map((route) => route.accountId)
        .filter((id) => typeof id === "string" && id),
    )) {
      try {
        accounts[id] = await providers.resolveClaudeAccount(id);
      } catch {
        // An account without a saved value is omitted and uses the host login.
      }
    }
    await orchdRequest(
      this.socketPath,
      "secrets.set",
      {
        profiles,
        accounts,
        projects: this.getProjects
          ? await this.getProjects()
              .agentEnvironments()
              .catch(() => ({}))
          : {},
        projectMcp: this.getProjects
          ? await this.getProjects()
              .mcpEnvironments()
              .catch(() => ({}))
          : {},
      },
      this.token,
      5000,
    ).catch(() => {});
  }

  async call(method, params = {}) {
    if (!ALLOWED_METHODS.has(method))
      throw new Error("Invalid orchestrator request");
    await this.#ensureRunning();
    // The first real request starts the subscribe loop, which from then on
    // may respawn a daemon that went away.
    this.used = true;
    if (!this.looping && !this.closed) this.connect();
    const sendParams =
      // A task the owner or the orchestrator agent creates gets the
      // project's own MCP servers, resolved here where the config lives.
      method === "task.create" ||
      method === "chat.send" ||
      method === "chat.edit"
        ? await this.#withMcp(params)
        : params;
    if (method === "task.create") await this.#pushSecrets();
    const result = await orchdRequest(
      this.socketPath,
      method,
      sendParams,
      this.token,
      // Pushing and opening a pull request can take minutes.
      method === "task.pr" ? 600000 : 20000,
    );
    // Awaited (not fire-and-forget) so a caller who follows this with another
    // settings-dependent call never races the push.
    if (method === "settings.set") await this.#pushSecrets().catch(() => {});
    return this.remote && TASK_RESULT_METHODS.has(method)
      ? tagTasks(result, this.host)
      : result;
  }

  /** A liveness check with no side effects: a ping on the socket already in
   * use. It never spawns, restarts or provisions a daemon, so a panel polling
   * it cannot bypass the reconnect backoff; a call or Retry does that. */
  async probe() {
    if (this.closed) throw new Error("orchestrator closed");
    if (!this.socketPath) throw new Error("The orchestrator is not connected.");
    return orchdRequest(this.socketPath, "ping", {}, this.token, 2000);
  }

  /** Starts relaying a daemon that is already running; never spawns one and
   * does nothing when there is none. A remote host connects on first use. */
  async attach() {
    if (this.remote) return;
    this.closed = false;
    const epoch = this.epoch;
    try {
      await this.#pingRunning();
    } catch {
      return;
    }
    if (!this.closed && epoch === this.epoch && !this.looping) this.connect();
  }

  /** True when a task on disk is not finished: such a task needs the daemon
   * (landing retries, autopilot, the badge), so launch starts it. Reads only
   * each small task.json; a remote host never scans. */
  async hasPendingTasks() {
    if (this.remote || !this.dataDir) return false;
    const root = path.join(this.dataDir, "tasks");
    let names;
    try {
      names = await fs.readdir(root);
    } catch {
      return false;
    }
    for (const name of names) {
      try {
        const file = path.join(root, name, "task.json");
        if ((await fs.stat(file)).size > 1_000_000) continue;
        const { status, archived } = JSON.parse(
          await fs.readFile(file, "utf8"),
        );
        if (
          typeof status === "string" &&
          !archived &&
          !FINISHED_STATUSES.has(status)
        )
          return true;
      } catch {
        // No task.json or unreadable: not a pending task.
      }
    }
    return false;
  }

  /** Treats launch as the first use: starts the daemon (or attaches) and the
   * event relay. */
  async resume() {
    await this.#ensureRunning();
    this.used = true;
    if (!this.looping && !this.closed) this.connect();
  }

  async #pingRunning() {
    await this.#refreshToken();
    return orchdRequest(this.socketPath, "ping", {}, this.token, 2000);
  }

  connect() {
    this.closed = false;
    this.epoch += 1;
    clearTimeout(this.retryTimer);
    this.remote?.reopen();
    this.#subscribeLoop(this.epoch);
  }

  async #subscribeLoop(epoch) {
    if (this.closed || epoch !== this.epoch) return;
    this.looping = true;
    try {
      if (this.used) await this.#ensureRunning();
      else await this.#pingRunning();
      await this.#pushSecrets();
      await this.#subscribeOnce();
    } catch {
      // Daemon not built / not reachable yet: retry with backoff below - but
      // an attach-only loop just stops, the first real request starts it again.
      if (!this.used) {
        if (epoch === this.epoch) this.looping = false;
        return;
      }
    }
    if (this.closed || epoch !== this.epoch) return;
    this.backoff = Math.min(this.backoff * 2, this.remote ? 60000 : 15000);
    this.retryTimer = setTimeout(
      () => this.#subscribeLoop(epoch),
      this.backoff,
    );
    this.retryTimer.unref?.();
  }

  /** The transition detector: raises one notice per (task, question) for a
   * task needing input, and one per (task, status) for a top-level task that
   * finished, failed, waits to land or was stopped by the engine. Subtasks never raise those, archived tasks
   * raise nothing. */
  #notifyTransition(message) {
    if (message.event !== "task") return;
    // `send` already tagged the relayed copy; the notice path needs it too.
    const task =
      this.remote && message.task && typeof message.task === "object"
        ? { ...message.task, host: this.host }
        : message.task;
    if (task && typeof task === "object") this.onTask?.(task);
    let previous;
    if (task && typeof task === "object" && task.id !== undefined) {
      previous = this.lastStatus.get(task.id);
      this.lastStatus.set(task.id, task.status);
    }
    if (!this.notify) return;
    if (!task || typeof task !== "object" || task.archived) return;
    const changed = previous !== undefined && previous !== task.status;
    let key;
    if (task.status === "waiting")
      key = `input:${task.id}:${task.question?.text || ""}`;
    else if (task.status === "done" && !task.parent)
      key = `${task.status}:${task.id}`;
    else if (
      (task.status === "failed" || task.status === "landing") &&
      changed &&
      !task.parent
    )
      key = `${task.status}:${task.id}`;
    else if (stoppedByEngine(task) && changed && !task.parent)
      key = `stopped:${task.id}`;
    if (!key || this.notified.has(key)) return;
    this.notified.add(key);
    this.notify(orchestratorNotice(task));
  }

  #subscribeOnce() {
    return new Promise((resolve) => {
      const socket = net.createConnection(this.socketPath);
      this.subscribeSocket = socket;
      let buffer = "";
      socket.setEncoding("utf8");
      socket.on("connect", () => {
        this.backoff = 500;
        const envelope = { id: "subscribe", method: "subscribe" };
        if (this.token) envelope.auth = this.token;
        socket.write(JSON.stringify(envelope) + "\n");
      });
      socket.on("data", (chunk) => {
        buffer += chunk;
        let boundary;
        while ((boundary = buffer.indexOf("\n")) >= 0) {
          const line = buffer.slice(0, boundary);
          buffer = buffer.slice(boundary + 1);
          if (!line.trim()) continue;
          let message;
          try {
            message = JSON.parse(line);
          } catch {
            continue;
          }
          if (!message.event) continue;
          this.send("orchestrator-event", message);
          this.#notifyTransition(message);
        }
      });
      const done = () => {
        if (this.subscribeSocket === socket) this.subscribeSocket = null;
        resolve();
      };
      socket.on("error", done);
      socket.on("close", done);
    });
  }

  // Closes only this app's subscribe connection, never the daemon: tasks
  // keep running when the window closes. quit() below is what stops one.
  close() {
    this.closed = true;
    this.looping = false;
    this.epoch += 1;
    clearTimeout(this.retryTimer);
    this.retryTimer = null;
    this.subscribeSocket?.destroy();
    this.remote?.close();
  }

  // What close() does, plus stopping the local daemon this app spawned (or,
  // in a test launch, any it found): `shutdown`, then SIGKILL if it lingers.
  // Never throws.
  async quit() {
    this.close();
    // A remote daemon is never stopped by the app, nor one it did not spawn.
    if (this.remote || !(this.spawned || this.stopDaemonOnQuit)) return;
    this.spawned = false;
    try {
      await this.running?.catch(() => {});
      await this.starting?.catch(() => {});
      await this.#refreshToken();
      let pid;
      try {
        pid = (
          await orchdRequest(this.socketPath, "ping", {}, this.token, 1000)
        )?.pid;
      } catch {
        // Daemon not reachable; fall back to the pidfile below.
      }
      if (!pid) {
        const raw = await fs
          .readFile(path.join(this.dataDir, "orchd.pid"), "utf8")
          .catch(() => "");
        pid = Number.parseInt(raw, 10) || undefined;
      }
      await orchdRequest(
        this.socketPath,
        "shutdown",
        {},
        this.token,
        this.quitTimeoutMs,
      ).catch(() => {});
      if (!pid) return;
      await waitForExit(pid, { timeoutMs: this.quitTimeoutMs });
      try {
        process.kill(pid, 0);
        process.kill(pid, "SIGKILL");
      } catch {
        // Already gone.
      }
    } catch {
      // Quitting must never fail on the daemon.
    }
  }
}

/** Every orchd the app talks to: the local daemon plus one per SSH profile
 * the owner has used the Orchestrator on. A saved remote host is known at
 * launch but connects on its first request. */
class OrchestratorHosts {
  constructor({
    local,
    connections,
    artifacts,
    hostsFile,
    createService,
    onChange,
    enabled = true,
  }) {
    this.on = enabled;
    this.connected = false;
    this.pending = Promise.resolve();
    this.local = local;
    this.getConnections = connections;
    this.artifacts = artifacts;
    this.hostsFile = hostsFile;
    this.createService = createService;
    this.onChange = onChange;
    this.services = new Map();
    this.enabled = new Set();
  }

  async #save() {
    if (!this.hostsFile) return;
    await fs.writeFile(this.hostsFile, JSON.stringify([...this.enabled]), {
      mode: 0o600,
    });
  }

  /** Turns the whole orchestrator on or off without a restart. Off closes the
   * every remote service and stops the local daemon this app spawned, exactly
   * as on quit; on attaches to a daemon that is already running and lists the
   * saved SSH hosts, connecting nothing else. Calls run one after another. */
  setEnabled(value) {
    const next = this.pending.then(async () => {
      if (value === this.on && value === this.connected) return;
      this.on = value;
      if (!value) {
        await this.quit();
        this.services.clear();
        this.connected = false;
        return;
      }
      this.connected = true;
      await this.local.attach();
      await this.init();
    });
    this.pending = next.catch(() => {});
    return next;
  }

  #assertOn() {
    if (!this.on) throw offError();
  }

  /** Lists the hosts the owner enabled earlier; they connect on first use.
   * Never throws. */
  async init() {
    if (!this.hostsFile) return;
    try {
      const saved = JSON.parse(await fs.readFile(this.hostsFile, "utf8"));
      for (const host of Array.isArray(saved) ? saved : [])
        if (typeof host === "string") this.#serviceFor(host, false);
    } catch {
      // No file yet, or unreadable: nothing was enabled.
    }
  }

  #serviceFor(host, persist = true) {
    this.#assertOn();
    if (host === LOCAL_HOST) return this.local;
    const existing = this.services.get(host);
    if (existing) return existing;
    const connections = this.getConnections();
    connections.get(host); // throws for an unknown / deleted profile
    const service = this.createService(host);
    this.services.set(host, service);
    this.enabled.add(host);
    if (persist) void this.#save().catch(() => {});
    this.onChange?.();
    return service;
  }

  /** Local plus every SSH profile, with what the panel shows about each. */
  list() {
    this.#assertOn();
    const profiles = this.getConnections?.()?.list() ?? [];
    return [
      { id: LOCAL_HOST, name: "Local", state: "ready", enabled: true },
      ...profiles.map((profile) => {
        const id = `ssh:${profile.id}`;
        const remote = this.services.get(id)?.remote;
        return {
          id,
          name: profile.name,
          state: remote?.state ?? "idle",
          detail: remote?.detail ?? "",
          enabled: this.services.has(id),
          preflight: remote?.preflight ?? null,
          platform: remote?.platform || undefined,
          orchdInstalled: remote?.orchdInstalled ?? undefined,
        };
      }),
    ];
  }

  async call(method, params, host = LOCAL_HOST) {
    if (typeof host !== "string") throw new Error("Invalid orchestrator host");
    return this.#serviceFor(host).call(method, params);
  }

  /** A side-effect-free ping of a host already connected; never enables or
   * provisions one. */
  async probe(host = LOCAL_HOST) {
    this.#assertOn();
    if (typeof host !== "string") throw new Error("Invalid orchestrator host");
    const service = host === LOCAL_HOST ? this.local : this.services.get(host);
    if (!service) throw new Error("The orchestrator host is not connected.");
    return service.probe();
  }

  /** Re-runs the host's preflight (git, claude, codex). */
  async preflight(host) {
    if (host === LOCAL_HOST) return null;
    const service = this.#serviceFor(host);
    service.remote.forget();
    await service.remote.ensure();
    return service.remote.refreshPreflight();
  }

  /** The owner's "Install Rust and set up": provisions the host, installing
   * Rust there first when orchd has to be built and cargo is missing. */
  async setup(host) {
    if (typeof host !== "string" || host === LOCAL_HOST)
      throw new Error("Invalid orchestrator host");
    const service = this.#serviceFor(host);
    await service.remote.setup();
    return service.remote.refreshPreflight();
  }

  close() {
    this.local.close();
    for (const service of this.services.values()) service.close();
  }

  async quit() {
    for (const service of this.services.values()) await service.quit();
    await this.local.quit();
  }
}

/** The local service plus the remote-host machinery, not yet connected. */
function createOrchestratorHosts({
  send,
  notify,
  onTask,
  dataDir,
  root,
  resourcesPath,
  packaged,
  getClaudeMcp,
  getModelProviders,
  getProjects,
  stopDaemonOnQuit,
  getConnections,
  hostsFile,
  hostsChanged,
  spawnRetries,
  stopWaitSeconds,
  spawnIntervalMs,
  enabled,
}) {
  const local = new OrchestratorService({
    dataDir,
    root,
    resourcesPath,
    packaged,
    send,
    notify,
    onTask,
    getClaudeMcp,
    getModelProviders,
    getProjects,
    stopDaemonOnQuit,
  });
  const artifacts = localArtifacts({
    root,
    binary: orchdBinaryPath({ root, resourcesPath, packaged }),
    resourcesPath,
    packaged,
  });
  return new OrchestratorHosts({
    local,
    connections: getConnections ?? (() => null),
    artifacts,
    hostsFile,
    onChange: hostsChanged,
    enabled,
    createService: (host) =>
      new OrchestratorService({
        host,
        send,
        notify,
        onTask,
        getProjects: undefined,
        remote: new RemoteOrchd({
          connections: getConnections(),
          endpoint: host,
          artifacts,
          request: orchdRequest,
          onChange: hostsChanged,
          spawnRetries,
          stopWaitSeconds,
          spawnIntervalMs,
        }),
      }),
  });
}

/** Registers the IPC surface. Nothing connects here: `start()` (called once
 * the connections exist) reads the extension's saved on/off state, and every
 * later toggle from `extensions` connects or closes the orchestrator. */
function registerOrchestratorExtension({ handle, extensions, ...options }) {
  const hosts = createOrchestratorHosts({ ...options, enabled: false });
  handle("orchestrator", (method, params, host) =>
    hosts.call(method, params, host),
  );
  handle("orchestrator-hosts", () => hosts.list());
  handle("orchestrator-preflight", (host) => hosts.preflight(host));
  handle("orchestrator-host-setup", (host) => hosts.setup(host));
  handle("orchestrator-probe", (host) => hosts.probe(host));
  const apply = () =>
    hosts
      .setEnabled(extensions.isEnabled(ORCHESTRATOR_MANIFEST.id))
      .catch(() => {});
  let started = false;
  extensions.onChange((id) => {
    if (started && id === ORCHESTRATOR_MANIFEST.id) return apply();
  });
  hosts.start = async () => {
    await extensions.ready;
    started = true;
    await apply();
    // Re-check the switch after the scan: it may have been turned off meanwhile.
    if (hosts.on && (await hosts.local.hasPendingTasks()) && hosts.on)
      await hosts.local.resume().catch(() => {});
  };
  return hosts;
}

module.exports = {
  ORCHESTRATOR_MANIFEST,
  OrchestratorService,
  OrchestratorHosts,
  registerOrchestratorExtension,
  createOrchestratorHosts,
  OFF_MESSAGE,
  orchdRequest,
  tagTasks,
  tagEvent,
  LOCAL_HOST,
  orchestratorNotice,
  orchdBinaryPath,
  socketPathFor,
  isStalePing,
  waitForExit,
  ALLOWED_METHODS,
  NOT_BUILT,
  NOT_INSTALLED,
  notBuiltMessage,
  FAILED_TO_START,
};
