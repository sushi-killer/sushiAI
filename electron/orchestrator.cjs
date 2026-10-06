// The Orchestrator's desktop side. The orchestration pipeline lives in the
// sushiai daemon as the `orch` module: every request here is `orch.<method>`
// on the daemon manager's connection to a host (the local daemon, or a remote
// one over `ssh host sushiai proxy`), and the module's `orch.event`
// notifications arrive per host through the same connection. This file owns
// the renderer allowlist, per-host event tagging and notices, the secrets the
// desktop pushes whenever a host becomes ready, and the host list. It starts,
// stops and installs nothing itself: the daemon manager connects hosts and the
// host installer provisions them.
const { appDb, transaction } = require("./app-db.cjs");
const { posixCommand, REMOTE_PATH, REMOTE_BIN } = require("./host-install.cjs");
const {
  PREFLIGHT_SCRIPT,
  parsePreflight,
  reconnectUntilReady,
} = require("./host-setup.cjs");

const LOCAL_HOST = "local";
const OFF_MESSAGE = "The orchestrator is off.";
const ORCH_CAPABILITY = "orch";
const ORCH_PREFIX = "orch.";
// The daemon answers `orch.*` with this code while the module starts.
const MODULE_STARTING = 1100;
// A host that is connecting gets this long to become ready.
const READY_TIMEOUT_MS = 30000;
// A host that is down is not reconnected on every request.
const RETRY_AFTER_MS = 30000;

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

/** The daemon manager's host name for an orchestrator host id. */
function managerHostOf(host) {
  return host === LOCAL_HOST ? LOCAL_HOST : host.replace(/^ssh:/, "");
}

function notReady(state, name) {
  const error = new Error(
    state?.message || `The sushiai daemon on ${name} is not ready.`,
  );
  error.code = "ORCH_HOST_NOT_READY";
  return error;
}

/** The error for a daemon that answers without the `orch` capability: it is
 * older than this app, or its Orchestrator module is not enabled. */
function missingCapability(name) {
  const error = new Error(`Update sushiai on ${name}`);
  error.code = "ORCH_MISSING";
  return error;
}

const hasOrch = (state) => !!state?.capabilities?.includes(ORCH_CAPABILITY);

/** The manager's state of `host` once it is not "connecting" (immediately when
 * it already is not). Resolves undefined for a host the manager does not know. */
function settledState(manager, host, timeoutMs, timeoutMessage) {
  const current = () => manager.states().find((state) => state.host === host);
  return new Promise((resolve, reject) => {
    let off = () => {};
    const done = (state) => {
      clearTimeout(timer);
      off();
      resolve(state);
    };
    const timer = setTimeout(() => {
      off();
      reject(new Error(timeoutMessage));
    }, timeoutMs);
    timer.unref?.();
    off = manager.on("state", (state) => {
      if (state.host === host && state.state !== "connecting") done(state);
    });
    // The state may have settled between the check and the subscription.
    const now = current();
    if (!now || now.state !== "connecting") done(now);
  });
}

/** The opt-in of the `orch` module on a host (owner O1). Enabling runs
 * `sushiai orch register` there (local: the bundled binary; remote: over the
 * existing ssh exec path), disabling runs `sushiai orch unregister`; then the
 * host's daemon restarts (`daemon.shutdown`, then reconnect) when it does not
 * serve the module yet or still does. Sessions survive: they live in their
 * holders. `host` is "local" or "ssh:<profile id>". */
function createModuleSwitch({
  getManager,
  runLocal,
  restartLocal,
  exec,
  readyTimeoutMs = READY_TIMEOUT_MS,
  settleMs,
  attempts,
}) {
  const enabling = new Map();

  async function apply(host, on) {
    const manager = getManager?.();
    if (!manager) throw new Error("The sushiai daemon is not running.");
    const name = managerHostOf(host);
    const verb = on ? "register" : "unregister";
    if (host === LOCAL_HOST) await runLocal(["orch", verb]);
    else
      await exec(
        host,
        posixCommand(`${REMOTE_PATH}\n"${REMOTE_BIN}/sushiai" orch ${verb}`),
        { timeout: 60000 },
      );
    const state = await settledState(
      manager,
      name,
      readyTimeoutMs,
      `The sushiai daemon on ${name} did not become ready.`,
    );
    if (state?.state !== "ready" || hasOrch(state) === on) return;
    if (host === LOCAL_HOST) await restartLocal();
    else await manager.request(name, "daemon.shutdown", {}).catch(() => {});
    // The daemon takes a moment to go; a retry that meets it still answers.
    await new Promise((resolve) => setTimeout(resolve, settleMs ?? 400));
    const after = await reconnectUntilReady(manager, name, {
      attempts,
      settleMs,
    });
    if (after?.state !== "ready" || hasOrch(after) !== on)
      throw new Error(
        `The sushiai daemon on ${name} did not restart ${on ? "with" : "without"} the orchestrator.`,
      );
  }

  return {
    /** Once per host until it is disabled; concurrent callers share the run. */
    enable(host) {
      let run = enabling.get(host);
      if (!run) {
        run = apply(host, true);
        enabling.set(host, run);
        run.catch(() => {
          if (enabling.get(host) === run) enabling.delete(host);
        });
      }
      return run;
    },
    async disable(host) {
      await enabling.get(host)?.catch(() => {});
      enabling.delete(host);
      await apply(host, false);
    },
  };
}

/** One host's orchestrator: requests as `orch.*` through the daemon manager,
 * the events of that host, its notices and its secrets. `host` is "local" or
 * "ssh:<profile id>"; the manager knows the host by `managerHostOf(host)`. */
class OrchestratorService {
  constructor({
    host = LOCAL_HOST,
    getManager,
    getConnections,
    send,
    notify,
    onTask,
    getClaudeMcp,
    getModelProviders,
    getProjects,
    moduleSwitch,
    readyTimeoutMs = READY_TIMEOUT_MS,
  }) {
    this.host = host;
    this.moduleSwitch = moduleSwitch;
    this.remote = host !== LOCAL_HOST;
    this.managerHost = managerHostOf(host);
    this.getManager = getManager;
    this.getConnections = getConnections;
    // A remote host's events carry the host so the renderer knows where a
    // task lives.
    this.send = this.remote
      ? (channel, message) => send(channel, tagEvent(message, host))
      : send;
    this.notify = notify;
    this.onTask = onTask;
    this.getClaudeMcp = getClaudeMcp;
    this.getModelProviders = getModelProviders;
    this.getProjects = getProjects;
    this.readyTimeoutMs = readyTimeoutMs;
    this.preflight = null;
    this.lastRetryAt = 0;
    // Keys of notices already raised (see #notifyTransition): a task
    // re-entering a state with the same question never re-notifies.
    this.notified = new Set();
    // Last status seen per task: failed/landing/stopped notify only on a
    // real change into that status, not when first seen already there.
    this.lastStatus = new Map();
  }

  get name() {
    if (!this.remote) return "this Mac";
    try {
      return this.getConnections().get(this.host).name;
    } catch {
      return this.managerHost;
    }
  }

  #manager() {
    const manager = this.getManager?.();
    if (!manager) throw new Error("The sushiai daemon is not running.");
    return manager;
  }

  #state() {
    return this.#manager()
      .states()
      .find((state) => state.host === this.managerHost);
  }

  /** The host's daemon state when it is ready and has the `orch` capability;
   * throws the reason it is not. Never connects anything. */
  #usable() {
    const state = this.#state();
    if (!state) throw new Error("The orchestrator host is not connected.");
    if (state.state !== "ready") throw notReady(state, this.name);
    if (!state.capabilities?.includes(ORCH_CAPABILITY))
      throw missingCapability(this.name);
    return state;
  }

  /** Waits for a connecting host to be ready. A remote host that is down is
   * reconnected (at most every RETRY_AFTER_MS): the first request on a saved
   * host is what connects it. Then checks the `orch` capability. */
  async ready() {
    const manager = this.#manager();
    let state = this.#state();
    if (!state) throw new Error("The orchestrator host is not connected.");
    if (
      this.remote &&
      state.state !== "ready" &&
      state.state !== "connecting" &&
      Date.now() - this.lastRetryAt >= RETRY_AFTER_MS
    ) {
      this.lastRetryAt = Date.now();
      state = await manager.retry(this.managerHost);
    }
    if (state.state === "connecting") state = await this.#whenSettled(manager);
    if (state?.state !== "ready") throw notReady(state, this.name);
    // A ready daemon without the module: the owner turned the Orchestrator on
    // after this host's daemon started, so the host is enabled now.
    if (!hasOrch(state) && this.moduleSwitch) {
      try {
        await this.moduleSwitch.enable(this.host);
      } catch (error) {
        throw new Error(
          `Could not enable the orchestrator on ${this.name}: ${error.message}`,
        );
      }
    }
    return this.#usable();
  }

  #whenSettled(manager) {
    return settledState(
      manager,
      this.managerHost,
      this.readyTimeoutMs,
      `The sushiai daemon on ${this.name} did not become ready.`,
    );
  }

  async #request(method, params, timeoutMs = 20000) {
    try {
      return await this.#manager().request(
        this.managerHost,
        ORCH_PREFIX + method,
        params,
        { timeoutMs },
      );
    } catch (error) {
      if (error?.code === MODULE_STARTING) {
        const starting = new Error(
          `The orchestrator on ${this.name} is still starting. Try again in a moment.`,
        );
        starting.code = "ORCH_STARTING";
        throw starting;
      }
      throw error;
    }
  }

  async #withMcp(params) {
    // The project's .mcp.json lives on this machine; a remote repo path has none here.
    if (typeof params?.repo !== "string") return params;
    if (this.getProjects) {
      try {
        const project = params.projectId
          ? await this.getProjects().get(params.projectId)
          : this.remote
            ? await this.getProjects().resolveFolder({
                endpoint: this.host,
                cwd: params.repo,
              })
            : await this.getProjects().resolveDirectory(params.repo);
        if (project) {
          params = { ...params, projectId: project.id };
          // The project keeps `{ mcpServers, disabledMcpServers }`: only the
          // servers that are on go to the task.
          const off = new Set(project.mcp?.disabledMcpServers || []);
          const own = Object.fromEntries(
            Object.entries(project.mcp?.mcpServers || {}).filter(
              ([name]) => !off.has(name),
            ),
          );
          if (Object.keys(own).length) {
            const current = params.mcp?.mcpServers || params.mcp || {};
            params.mcp = { mcpServers: { ...own, ...current } };
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

  /** Replaces what the daemon holds with what is allowed now (a host switched
   * off for a project gets no values of it). Never throws. */
  refreshSecrets() {
    return this.#pushSecrets().catch(() => {});
  }

  /** Full replace, always pushed (host ready, task.create, after every
   * `settings.set`): `profiles` is every route's resolved model-profile env +
   * key. Secrets live in the daemon's memory only, so a daemon restart loses
   * them and the next `ready` pushes them again. */
  async #pushSecrets() {
    if (!this.getProjects && (this.remote || !this.getModelProviders)) return;
    this.#usable();
    let settings;
    try {
      settings = await this.#request("settings.get", {}, 5000);
    } catch {
      return;
    }
    // Provider and subscription credentials stay on this machine. Project
    // values are resolved per host, which honours "don't send to this host".
    const providers =
      !this.remote && this.getModelProviders ? this.getModelProviders() : null;
    const routes = settings?.routes || [];
    const idsOf = (key) => [
      ...new Set(
        routes
          .map((route) => route[key])
          .filter((id) => typeof id === "string" && id),
      ),
    ];
    const profiles = {};
    const accounts = {};
    for (const id of providers ? idsOf("profileId") : []) {
      try {
        const { settings: env, key } = await providers.resolveEnv(id);
        profiles[id] = { env, key };
      } catch {
        // Missing key / deleted profile: omitted, so that route falls back
        // to the tier's plain route, per the profile-fallback contract.
      }
    }
    for (const id of providers ? idsOf("accountId") : []) {
      try {
        accounts[id] = await providers.resolveClaudeAccount(id);
      } catch {
        // An account without a saved value is omitted and uses the host login.
      }
    }
    const projects = this.getProjects?.();
    const target = this.remote ? this.host : "local";
    await this.#request(
      "secrets.set",
      {
        profiles,
        accounts,
        projects: projects
          ? await projects.agentEnvironments(target).catch(() => ({}))
          : {},
        projectMcp: projects
          ? await projects.mcpEnvironments(target).catch(() => ({}))
          : {},
        projectRepos: projects
          ? await Promise.resolve(projects.repoProjects?.(target))
              .then((map) => map ?? {})
              .catch(() => ({}))
          : {},
      },
      5000,
    );
  }

  async call(method, params = {}) {
    if (!ALLOWED_METHODS.has(method))
      throw new Error("Invalid orchestrator request");
    await this.ready();
    const sendParams =
      // A task the owner or the orchestrator agent creates gets the
      // project's own MCP servers, resolved here where the config lives.
      method === "task.create" ||
      method === "chat.send" ||
      method === "chat.edit"
        ? await this.#withMcp(params)
        : params;
    if (method === "task.create") await this.refreshSecrets();
    const result = await this.#request(
      method,
      sendParams,
      // Pushing and opening a pull request can take minutes.
      method === "task.pr" ? 600000 : 20000,
    );
    // Awaited (not fire-and-forget) so a caller who follows this with another
    // settings-dependent call never races the push.
    if (method === "settings.set") await this.refreshSecrets();
    return this.remote && TASK_RESULT_METHODS.has(method)
      ? tagTasks(result, this.host)
      : result;
  }

  /** A liveness check with no side effects: the host is ready and has the
   * `orch` capability. It never connects or provisions a host. */
  probe() {
    this.#usable();
    return { pid: 0 };
  }

  /** What the host offers a route's harness (git, claude, codex), read over
   * the host's SSH connection. A host with no shell has none. */
  async refreshPreflight() {
    if (!this.remote) return null;
    const connections = this.getConnections();
    if (!connections.hasShell(this.host)) return null;
    this.preflight = parsePreflight(
      await connections.exec(this.host, posixCommand(PREFLIGHT_SCRIPT), {
        timeout: 40000,
      }),
    );
    return this.preflight;
  }

  /** Forgets that the host was just retried: the owner's Try again. */
  forget() {
    this.lastRetryAt = 0;
  }

  /** Lagging subscribers are told to refetch: every task is replayed as a task
   * event, which is what the panel folds into its state. */
  async resync() {
    const tasks = await this.#request("task.list", {});
    for (const task of Array.isArray(tasks) ? tasks : [])
      this.handleEvent({ event: "task", task });
  }

  /** One `orch.event` notification of this host. */
  handleEvent(message) {
    if (!message || typeof message !== "object" || !message.event) return;
    this.send("orchestrator-event", message);
    this.#notifyTransition(message);
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
}

/** The daemon manager's state as the panel's host record. */
function hostRecord(id, name, state, enabled) {
  const base = { id, name, enabled };
  if (!state) return { ...base, state: "idle" };
  switch (state.state) {
    case "ready":
      return state.capabilities?.includes(ORCH_CAPABILITY)
        ? { ...base, state: "ready" }
        : { ...base, state: "error", detail: `Update sushiai on ${name}` };
    case "connecting":
      return { ...base, state: "connecting" };
    case "offline":
      return { ...base, state: "idle", detail: state.message || "" };
    default:
      return {
        ...base,
        state: "error",
        detail: state.message || state.hint || "The host is not reachable.",
      };
  }
}

/** Every host the app's orchestrator talks to: the local daemon plus one per
 * SSH profile the owner has used the Orchestrator on. A saved remote host is
 * known at launch but connects on its first request. */
class OrchestratorHosts {
  constructor({
    local,
    connections,
    userDataDir,
    createService,
    getManager,
    installHost,
    moduleSwitch,
    log = () => {},
    onChange,
    enabled = true,
  }) {
    this.on = enabled;
    this.moduleSwitch = moduleSwitch;
    this.log = log;
    this.connected = false;
    this.pending = Promise.resolve();
    this.local = local;
    this.getConnections = connections;
    this.userDataDir = userDataDir;
    this.createService = createService;
    this.getManager = getManager;
    this.installHost = installHost;
    this.onChange = onChange;
    this.services = new Map();
    this.enabled = new Set();
    this.loaded = false;
    this.unsubscribe = null;
  }

  #save() {
    if (!this.userDataDir) return;
    const db = appDb(this.userDataDir);
    transaction(db, () => {
      // Hosts that could not be loaded are not ours to forget.
      if (this.loaded) db.exec("DELETE FROM orchestrator_hosts");
      const insert = db.prepare(
        "INSERT OR IGNORE INTO orchestrator_hosts(host) VALUES(?)",
      );
      for (const host of this.enabled) insert.run(host);
    });
  }

  /** Turns the whole orchestrator on or off without a restart. Off forgets
   * the remote services and stops listening; the daemons keep running and no
   * task stops. On listens again, lists the saved SSH hosts and pushes the
   * secrets to every host that is ready. Calls run one after another. */
  setEnabled(value) {
    const next = this.pending.then(async () => {
      if (value === this.on && value === this.connected) return;
      this.on = value;
      if (!value) {
        const reached = this.#hostsToSwitch();
        this.close();
        this.services.clear();
        this.connected = false;
        await this.#switchModule(reached, false);
        return;
      }
      this.connected = true;
      this.#listen();
      await this.init();
      // Remote hosts are enabled on their first request; the Mac is now.
      await this.#switchModule([LOCAL_HOST], true);
      await this.refreshAllSecrets();
    });
    this.pending = next.catch(() => {});
    return next;
  }

  /** The local daemon plus every remote host that is connected now: a host
   * that is down is not reached over ssh just to be switched off. */
  #hostsToSwitch() {
    const states = this.getManager?.()?.states() ?? [];
    return [LOCAL_HOST, ...this.services.keys()].filter(
      (host) =>
        host === LOCAL_HOST ||
        states.some(
          (state) =>
            state.host === managerHostOf(host) && state.state === "ready",
        ),
    );
  }

  /** Registers or unregisters the module on `hosts`, never throwing: a host
   * that fails keeps the owner's toggle; its next request reports why. */
  async #switchModule(hosts, on) {
    if (!this.moduleSwitch) return;
    await Promise.all(
      hosts.map((host) =>
        (on
          ? this.moduleSwitch.enable(host)
          : this.moduleSwitch.disable(host)
        ).catch((error) =>
          this.log(
            `orchestrator ${on ? "enable" : "disable"} on ${host}: ${error.message}`,
          ),
        ),
      ),
    );
  }

  #assertOn() {
    if (!this.on) throw offError();
  }

  #serviceOfManagerHost(host) {
    return host === LOCAL_HOST ? this.local : this.services.get(`ssh:${host}`);
  }

  /** Subscribes to the manager once it exists: events per host, and the
   * secrets push when a host becomes ready. */
  #listen() {
    if (this.unsubscribe) return;
    const manager = this.getManager?.();
    if (!manager) return;
    const offEvent = manager.on("event", (event) => {
      if (!this.on) return;
      const service = this.#serviceOfManagerHost(event.host);
      if (!service) return;
      if (event.method === `${ORCH_PREFIX}event`)
        service.handleEvent(event.params);
      else if (event.method === "session.resync")
        void service.resync().catch(() => {});
    });
    const offState = manager.on("state", (state) => {
      if (!this.on) return;
      const service = this.#serviceOfManagerHost(state.host);
      this.onChange?.();
      if (state.state !== "ready" || !service) return;
      // A daemon that restarted lost the secrets held in its memory.
      void service.refreshSecrets();
      void service
        .refreshPreflight()
        .then(() => this.onChange?.())
        .catch(() => {});
    });
    this.unsubscribe = () => {
      offEvent();
      offState();
    };
  }

  /** Sending to `host` was switched off or on: its daemon forgets values it may no longer
   * hold. Only a service that already exists is touched. */
  refreshSecrets(host) {
    return this.services.get(host)?.refreshSecrets();
  }

  /** What the projects hold changed: every daemon this app talks to is told. */
  refreshAllSecrets() {
    return Promise.all(
      [this.local, ...this.services.values()].map((service) =>
        service?.refreshSecrets?.(),
      ),
    );
  }

  /** Lists the hosts the owner enabled earlier; they connect on first use.
   * Never throws. */
  async init() {
    if (!this.userDataDir) return;
    try {
      const rows = appDb(this.userDataDir)
        .prepare("SELECT host FROM orchestrator_hosts")
        .all();
      for (const { host } of rows) this.#serviceFor(host, false);
      this.loaded = true;
    } catch {
      // Unreadable store: nothing was enabled.
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
    if (persist)
      try {
        this.#save();
      } catch {
        // The host works this session; it just is not remembered.
      }
    this.onChange?.();
    return service;
  }

  /** Local plus every SSH profile, with what the panel shows about each. */
  list() {
    this.#assertOn();
    const states = this.getManager?.()?.states() ?? [];
    const stateOf = (host) => states.find((state) => state.host === host);
    const profiles = this.getConnections?.()?.list() ?? [];
    return [
      hostRecord(LOCAL_HOST, "Local", stateOf(LOCAL_HOST), true),
      ...profiles.map((profile) => {
        const id = `ssh:${profile.id}`;
        const service = this.services.get(id);
        return {
          ...hostRecord(id, profile.name, stateOf(profile.id), !!service),
          preflight: service?.preflight ?? null,
        };
      }),
    ];
  }

  async call(method, params, host = LOCAL_HOST) {
    if (typeof host !== "string") throw new Error("Invalid orchestrator host");
    return this.#serviceFor(host).call(method, params);
  }

  /** A side-effect-free check of a host already connected; never enables or
   * connects one. */
  async probe(host = LOCAL_HOST) {
    this.#assertOn();
    if (typeof host !== "string") throw new Error("Invalid orchestrator host");
    const service = host === LOCAL_HOST ? this.local : this.services.get(host);
    if (!service) throw new Error("The orchestrator host is not connected.");
    return service.probe();
  }

  /** The owner's Try again: reconnects the host and re-reads what it offers
   * (git, claude, codex). A host that stays unreachable answers null; its
   * state is on the host list. */
  async preflight(host) {
    if (host === LOCAL_HOST) return null;
    const service = this.#serviceFor(host);
    service.forget();
    await this.getManager?.()
      ?.retry(service.managerHost)
      .catch(() => {});
    this.onChange?.();
    return service.refreshPreflight().catch(() => null);
  }

  /** The owner's "Update sushiai": installs the bundled sushiai on the host
   * through the one host installer, which restarts the daemon there. */
  async setup(host) {
    if (typeof host !== "string" || host === LOCAL_HOST)
      throw new Error("Invalid orchestrator host");
    const service = this.#serviceFor(host);
    if (!this.installHost)
      throw new Error("Installing sushiai is unavailable.");
    await this.installHost(service.managerHost);
    return service.refreshPreflight().catch(() => null);
  }

  /** Stops listening. The daemons are never stopped: tasks keep running. */
  close() {
    this.unsubscribe?.();
    this.unsubscribe = null;
  }

  async quit() {
    this.close();
  }
}

/** The local service plus the remote-host machinery, not yet connected. */
function createOrchestratorHosts({
  send,
  notify,
  onTask,
  getClaudeMcp,
  getModelProviders,
  getProjects,
  getConnections,
  getManager,
  installHost,
  moduleSwitch,
  log,
  userDataDir,
  hostsChanged,
  readyTimeoutMs,
  enabled,
}) {
  const service = (host) =>
    new OrchestratorService({
      host,
      getManager,
      getConnections,
      send,
      notify,
      onTask,
      getClaudeMcp,
      getModelProviders,
      getProjects,
      moduleSwitch,
      readyTimeoutMs,
    });
  const hosts = new OrchestratorHosts({
    local: service(LOCAL_HOST),
    connections: getConnections ?? (() => null),
    userDataDir,
    getManager,
    installHost,
    moduleSwitch,
    log,
    onChange: hostsChanged,
    enabled,
    createService: service,
  });
  // When sending to a host is switched off or on, its daemon's copy of the
  // project values is replaced.
  const projects = getProjects?.();
  if (projects) {
    projects.onSendChange = (host) => hosts.refreshSecrets(host);
    // An edit in Project settings reaches every host's daemon at once.
    projects.onChange = () => hosts.refreshAllSecrets();
  }
  return hosts;
}

/** Registers the IPC surface. Nothing connects here: `start()` (called once
 * the daemon manager exists) reads the extension's saved on/off state, and
 * every later toggle from `extensions` listens or stops listening. */
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
  };
  return hosts;
}

module.exports = {
  ORCHESTRATOR_MANIFEST,
  OrchestratorService,
  OrchestratorHosts,
  registerOrchestratorExtension,
  createOrchestratorHosts,
  createModuleSwitch,
  OFF_MESSAGE,
  tagTasks,
  tagEvent,
  LOCAL_HOST,
  orchestratorNotice,
  managerHostOf,
  ALLOWED_METHODS,
};
