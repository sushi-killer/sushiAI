// The orchestrator daemon (`orchd/`, Rust) outlives the app: this module only
// finds/spawns it, speaks its NDJSON protocol, and relays its `subscribe`
// stream to the renderer. It never stops the daemon on a normal quit - a
// running task must survive both a window close and an app quit. The one
// exception is a test launch (`stopDaemonOnQuit`, set from SUSHIAI_TEST_WINDOW):
// its throwaway daemon is shut down on quit so it does not linger. orchd also
// exits on its own when its data dir is deleted.
const net = require("node:net");
const path = require("node:path");
const os = require("node:os");
const fs = require("node:fs/promises");
const { existsSync } = require("node:fs");
const { spawn } = require("node:child_process");
const { createHash, randomUUID } = require("node:crypto");

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
  "task.stop",
  "task.answer",
  "task.overturn",
  "task.leadTouch",
  "task.delete",
  "task.archive",
  "task.unarchive",
  "task.timeline",
  "task.evidence",
  "failures.catalogue",
  "costs.summary",
  "chat.get",
  "chat.send",
  "chat.cancel",
  "chat.list",
  "chat.new",
  "chat.switch",
  "chat.clear",
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

const NOT_BUILT =
  "The orchestrator daemon is not built. Run npm run build:orchd.";
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
  const at = Number(task.question?.askedAt) || Number(task.updatedAt) || 0;
  if (at > 0) notice.at = at;
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
    spawnRetries = 50,
    spawnIntervalMs = 100,
    stopDaemonOnQuit = false,
    quitTimeoutMs = 2000,
  }) {
    this.dataDir = dataDir;
    this.socketPath = socketPathFor(dataDir);
    this.binary = orchdBinaryPath({ root, resourcesPath, packaged });
    this.send = send;
    this.notify = notify;
    this.onTask = onTask;
    this.getClaudeMcp = getClaudeMcp;
    this.getModelProviders = getModelProviders;
    this.spawnRetries = spawnRetries;
    this.spawnIntervalMs = spawnIntervalMs;
    this.stopDaemonOnQuit = stopDaemonOnQuit;
    this.quitTimeoutMs = quitTimeoutMs;
    this.token = null;
    this.subscribeSocket = null;
    this.backoff = 500;
    this.closed = false;
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
    if (!existsSync(this.binary)) throw new Error(NOT_BUILT);
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
      ["--data", this.dataDir, "--socket", this.socketPath],
      { detached: true, stdio: "ignore" },
    );
    // Detached and unref'd on purpose: this process must outlive the app.
    child.unref();
    for (let attempt = 0; attempt < this.spawnRetries; attempt++) {
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
    if (!this.getClaudeMcp || typeof params?.repo !== "string") return params;
    try {
      const mcp = await this.getClaudeMcp().launchConfig(params.repo);
      return { ...params, mcp };
    } catch {
      // No .mcp.json / no readable project: the task still starts, just
      // without MCP servers wired into the Claude run.
      return params;
    }
  }

  /** Full replace, always pushed (connect + after every `settings.set`):
   * `classifier` is the selected provider's key/baseUrl or null, `profiles`
   * is every route's resolved model-profile env + key. Nothing is staged to
   * disk - `resolveEnv` hands the env map and key back in memory. */
  async #pushSecrets() {
    if (!this.getModelProviders) return;
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
    let classifier = null;
    const providerId = settings?.classifier?.providerId;
    if (providerId) {
      try {
        const [key, list] = await Promise.all([
          providers.keyFor(providerId),
          providers.listProviders(),
        ]);
        const provider = list.find((p) => p.id === providerId);
        if (key && provider) classifier = { key, baseUrl: provider.baseUrl };
      } catch {
        classifier = null;
      }
    }
    const profileIds = [
      ...new Set(
        (settings?.routes || [])
          .map((route) => route.profileId)
          .filter((id) => typeof id === "string" && id),
      ),
    ];
    const profiles = {};
    for (const id of profileIds) {
      try {
        const { settings: env, key } = await providers.resolveEnv(id);
        profiles[id] = { env, key };
      } catch {
        // Missing key / deleted profile: omitted, so that route falls back
        // to the tier's plain route, per the profile-fallback contract.
      }
    }
    await orchdRequest(
      this.socketPath,
      "secrets.set",
      { classifier, profiles },
      this.token,
      5000,
    ).catch(() => {});
  }

  async call(method, params = {}) {
    if (!ALLOWED_METHODS.has(method))
      throw new Error("Invalid orchestrator request");
    await this.#ensureRunning();
    const sendParams =
      // A task the owner or the orchestrator agent creates gets the
      // project's own MCP servers, resolved here where the config lives.
      method === "task.create" || method === "chat.send"
        ? await this.#withMcp(params)
        : params;
    const result = await orchdRequest(
      this.socketPath,
      method,
      sendParams,
      this.token,
      20000,
    );
    // Awaited (not fire-and-forget) so a caller who follows this with another
    // settings-dependent call never races the push.
    if (method === "settings.set") await this.#pushSecrets().catch(() => {});
    return result;
  }

  connect() {
    this.closed = false;
    this.#subscribeLoop();
  }

  async #subscribeLoop() {
    if (this.closed) return;
    try {
      await this.#ensureRunning();
      await this.#pushSecrets();
      await this.#subscribeOnce();
    } catch {
      // Daemon not built / not reachable yet: retry with backoff below.
    }
    if (this.closed) return;
    this.backoff = Math.min(this.backoff * 2, 15000);
    setTimeout(() => this.#subscribeLoop(), this.backoff).unref?.();
  }

  /** The transition detector: raises one notice per (task, question) for a
   * task needing input, and one per (task, status) for a top-level task that
   * finished, failed, waits to land or was stopped by the engine. Subtasks never raise those, archived tasks
   * raise nothing. */
  #notifyTransition(message) {
    if (message.event !== "task") return;
    const task = message.task;
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
  // must keep running after the window (or the app) closes. The one exception
  // is a test launch, which stops its throwaway daemon in quit() below.
  close() {
    this.closed = true;
    this.subscribeSocket?.destroy();
  }

  // What close() does, plus - only for a test launch (`stopDaemonOnQuit`) -
  // stopping the throwaway daemon: `shutdown`, then SIGKILL if it lingers.
  // Never throws.
  async quit() {
    this.close();
    if (!this.stopDaemonOnQuit) return;
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

function registerOrchestratorExtension({
  handle,
  send,
  notify,
  onTask,
  dataDir,
  root,
  resourcesPath,
  packaged,
  getClaudeMcp,
  getModelProviders,
  stopDaemonOnQuit,
}) {
  const service = new OrchestratorService({
    dataDir,
    root,
    resourcesPath,
    packaged,
    send,
    notify,
    onTask,
    getClaudeMcp,
    getModelProviders,
    stopDaemonOnQuit,
  });
  handle("orchestrator", (method, params) => service.call(method, params));
  service.connect();
  return service;
}

module.exports = {
  ORCHESTRATOR_MANIFEST,
  OrchestratorService,
  registerOrchestratorExtension,
  orchestratorNotice,
  orchdBinaryPath,
  socketPathFor,
  isStalePing,
  waitForExit,
  ALLOWED_METHODS,
  NOT_BUILT,
  FAILED_TO_START,
};
