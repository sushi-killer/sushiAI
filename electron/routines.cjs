"use strict";

// The Routines module's desktop side. The routines live in the sushiai daemon
// of each host as the `routines` module: every request here is
// `routines.<method>` on the daemon manager's connection to that host, and the
// module's `routines.event` notifications arrive per host through the same
// connection. This file owns the renderer allowlist (module IPC), the catalog
// the daemon validates against, the builder session, event forwarding and the
// on/off switch of the module on the hosts. It starts and installs nothing
// else: the daemon manager connects hosts and the host installer provisions
// them.
const os = require("node:os");
const path = require("node:path");
const fs = require("node:fs");
const { randomUUID } = require("node:crypto");
const {
  ROUTINES_MANIFEST,
  ROUTINES_SKILL,
} = require("./extensions/builtin-routines.cjs");
const { assembleCatalog, daemonCatalog } = require("./routines-catalog.cjs");
const { harnessFor, isHarness } = require("./harness.cjs");

const CAPABILITY = "routines";
const EVENT = "routines.event";
const SETTINGS_STORE = "routines-settings";
/** Hosts that served the module when Routines was turned off: they get it back
 * when Routines is turned on (an ssh host's schedules would stay hidden). */
const RETIRED_STORE = "routines-retired-hosts";
const LOCAL = "local";
const SKILL = ROUTINES_SKILL.name;
const OFF_MESSAGE = "Routines are off.";
// The probe of a host (login, models) is reused for this long.
const FACTS_TTL_MS = 60000;

// What the renderer reaches. `suspend` is ours alone.
const DAEMON_METHODS = new Set([
  "list",
  "show",
  "save",
  "validate",
  "set_enabled",
  "run_now",
  "dry_run",
  "runs",
  "result",
  "delete",
  "grant",
  "revoke",
  "approve",
]);
// Methods that validate against the catalog: the desktop sends the one it
// assembled (login state and models of the routine's host).
const WITH_CATALOG = new Set(["save", "validate", "set_enabled", "dry_run"]);

const ACCESS_MODES = ["cli", "mcp", "both"];

const DEFAULT_SETTINGS = Object.freeze({
  harness: "claude",
  model: "",
  effort: "medium",
  host: LOCAL,
  accountId: "",
  // What the builder may look at, and how it reaches the sushiai commands.
  sees: Object.freeze({ projects: true, connections: true }),
  access: "cli",
});

/** The server the builder attaches for `access: "mcp" | "both"`. */
const MCP_SERVER = "sushiai";
const MCP_COMMAND = "sushiai";
const MCP_ARGS = ["mcp", "--routines"];
const MCP_TOOLS = [
  "routines_projects",
  "routines_hosts",
  "routines_skills",
  "routines_list",
  "routines_show",
  "routines_validate",
  "routines_save",
  "routines_dry_run",
];

function offError() {
  const error = new Error(OFF_MESSAGE);
  error.code = "ROUTINES_OFF";
  return error;
}

/** "local" or "ssh:<profile id>". */
function validHost(value) {
  return (
    typeof value === "string" &&
    (value === LOCAL || /^ssh:.+/.test(value)) &&
    value.length <= 256
  );
}
const managerHostOf = (host) =>
  host === LOCAL ? LOCAL : host.replace(/^ssh:/, "");
const hostIdOf = (managerHost) =>
  managerHost === LOCAL ? LOCAL : `ssh:${managerHost}`;

/** The saved settings of the builder, repaired to something launchable.
 * `agents` is this Mac's agent catalog (keyed by agent id); with no catalog the
 * effort is kept, so a level the CLI refuses is never invented here. */
function builderSettings(stored, agents) {
  const value = { ...DEFAULT_SETTINGS, ...(stored || {}) };
  if (!isHarness(value.harness)) value.harness = DEFAULT_SETTINGS.harness;
  const levels = agents?.[value.harness]?.builderEfforts;
  if (levels?.length && !levels.includes(value.effort))
    value.effort = DEFAULT_SETTINGS.effort;
  value.model = typeof value.model === "string" ? value.model : "";
  value.accountId = typeof value.accountId === "string" ? value.accountId : "";
  value.host = validHost(value.host) ? value.host : LOCAL;
  const sees =
    value.sees && typeof value.sees === "object" && !Array.isArray(value.sees)
      ? value.sees
      : {};
  value.sees = {
    projects:
      typeof sees.projects === "boolean"
        ? sees.projects
        : DEFAULT_SETTINGS.sees.projects,
    connections:
      typeof sees.connections === "boolean"
        ? sees.connections
        : DEFAULT_SETTINGS.sees.connections,
  };
  if (!ACCESS_MODES.includes(value.access))
    value.access = DEFAULT_SETTINGS.access;
  // Codex's sandbox refuses the daemon socket, so only its MCP server (spawned
  // outside the sandbox) can reach the daemon. The stored value stays for Claude.
  if (value.harness === "codex" && value.access === "cli") value.access = "mcp";
  return value;
}

const usesMcp = (settings) => ["mcp", "both"].includes(settings.access);

/** The harness flags of the builder session: its effort and the limits of its
 * tools. The builder reads, writes its own folder and runs sushiai. */
function builderArgs(settings) {
  return harnessFor(settings.harness).headlessTurnArgs({
    effort: settings.effort,
    mcp: usesMcp(settings)
      ? {
          name: MCP_SERVER,
          command: MCP_COMMAND,
          args: MCP_ARGS,
          tools: MCP_TOOLS,
        }
      : null,
  });
}

/** The first prompt of a builder session: the owner's words, the limits the
 * owner set in Settings, then the skill. */
function builderPrompt(text, routineId, settings = DEFAULT_SETTINGS) {
  const task = routineId
    ? `Change the existing routine "${routineId}" as follows.`
    : "Build a routine from this request.";
  const limits = [];
  if (settings.sees?.projects === false)
    limits.push(
      "Do not read the owner's project files and do not run `sushiai projects show`; " +
        "pick a project only when the owner names it.",
    );
  if (settings.sees?.connections === false)
    limits.push(
      "Do not propose connector or MCP tools in `access`; use only built-in tools " +
        "and the sushiai commands.",
    );
  if (settings.access === "mcp")
    limits.push(
      `Use the ${MCP_SERVER} MCP tools (${MCP_TOOLS.join(", ")}) instead of the sushiai commands to read and build.`,
    );
  else if (settings.access === "both")
    limits.push(
      `The ${MCP_SERVER} MCP tools (${MCP_TOOLS.join(", ")}) and the sushiai commands do the same; use either.`,
    );
  const rules = limits.length
    ? `\n\nOwner's limits for this build:\n${limits.map((l) => `- ${l}`).join("\n")}`
    : "";
  const turn =
    "\n\nThis is a non-interactive turn: do not use `sushiai ask`. When you " +
    "need an answer from the owner, end your turn with that one question; " +
    "the owner replies in the next turn. Reply in the language of the owner's " +
    "request: your messages, questions, ask-card labels and options, and the " +
    "routine name and summary the owner sees; keep routine.json keys, ids, " +
    "cron and the brief structure as the skill defines. Your own reply is " +
    "short plain prose for the owner (plus at most one ask block); never " +
    "print the run result JSON, that is only for the routine's own run. If the tools you need are missing " +
    "(`sushiai` is not found or the " +
    `${MCP_SERVER} MCP tools are not available), do not search the disk or ` +
    "apologise at length: end the turn with the single sentence " +
    '"sushiai is not available in this session." and nothing else.';
  return `${task}\n\n${String(text).trim()}${rules}${turn}\n\nUse the skill ${SKILL} and follow it.`;
}

/** The command of a login terminal: the harness's own login, run through a
 * login shell (bash when the host has it), because an agent installed under
 * ~/.local/bin is on the login shell's PATH only. When the harness is still
 * not found the owner lands in a shell and sees why. */
function loginTerminalCommand(harness) {
  const words = harnessFor(harness).login;
  const inner = `${words.join(" ")}; exec "\${SHELL:-/bin/sh}" -l`;
  return [
    "/bin/sh",
    "-c",
    'if command -v bash >/dev/null 2>&1; then exec bash -lc "$1"; fi; exec sh -lc "$1"',
    "sh",
    inner,
  ];
}

/** The app data dir when running inside Electron, else null. */
function appDataDir() {
  try {
    const app = require("electron")?.app;
    return typeof app?.getPath === "function" ? app.getPath("userData") : null;
  } catch {
    return null;
  }
}

const REMOTE_BUILDER_DIR = ".sushiai/routines/_builder";

function createRoutines({
  getManager,
  getConnections,
  getProjects,
  isEnabled,
  moduleSwitch = null,
  send,
  readStore,
  writeStore,
  // What each host's daemon says its agent CLIs offer (`agent-catalog.cjs`).
  // A remote host's models come from that host's daemon, never over ssh.
  agentCatalog: agents,
  probeHost,
  standardSettings = async () => null,
  launch,
  userDataDir = appDataDir(),
  builderCwd = null,
  turnPollMs = 250,
  turnLimit = 30 * 60 * 1000,
  log = () => {},
  now = () => Date.now(),
}) {
  const factsCache = new Map();
  let unsubscribe = null;

  const assertOn = () => {
    if (!isEnabled()) throw offError();
  };
  const manager = () => {
    const value = getManager?.();
    if (!value) throw new Error("The sushiai daemon is not running.");
    return value;
  };
  const stateOf = (hostId) =>
    manager()
      .states()
      .find((state) => state.host === managerHostOf(hostId));
  const hasModule = (state) => !!state?.capabilities?.includes(CAPABILITY);

  /** Local plus every saved ssh profile, as the page lists them. */
  function hostList() {
    const states = getManager?.()?.states() ?? [];
    const profiles = getConnections?.()?.list() ?? [];
    const row = (id, name) => {
      const state = states.find((item) => item.host === managerHostOf(id));
      const hello =
        state?.state === "ready" ? manager().hello?.(state.host) : null;
      return {
        id,
        name,
        state: state?.state ?? "offline",
        routines: hasModule(state),
        hostName: hello?.host || (id === LOCAL ? LOCAL : ""),
      };
    };
    return [
      row(LOCAL, "This Mac"),
      ...profiles
        .filter((profile) => profile.id !== LOCAL)
        .map((profile) => row(`ssh:${profile.id}`, profile.name)),
    ];
  }

  async function facts(hostId) {
    const cached = factsCache.get(hostId);
    if (cached && now() - cached.at < FACTS_TTL_MS) return cached.value;
    let value = null;
    try {
      value = (await probeHost(hostId)) || null;
    } catch (error) {
      log(`routines probe ${hostId}: ${error.message}`);
    }
    factsCache.set(hostId, { at: now(), value });
    return value;
  }

  async function catalog() {
    const hosts = hostList();
    const entries = await Promise.all(
      hosts
        .filter((host) => host.state === "ready")
        .map(async (host) => [host.id, await facts(host.id)]),
    );
    const [projects, settings] = await Promise.all([
      getProjects().list(),
      standardSettings().catch(() => null),
    ]);
    const offered = Object.fromEntries(
      await Promise.all(
        hosts
          .filter((host) => host.state === "ready")
          .map(async (host) => [
            host.id,
            await agents.of(managerHostOf(host.id)),
          ]),
      ),
    );
    return assembleCatalog({
      hosts,
      facts: Object.fromEntries(entries.filter(([, value]) => value)),
      catalogs: offered,
      projects,
      settings,
    });
  }

  /** The host's daemon serves the module, switching it on first when the
   * owner has Routines on but this host never served it. */
  async function ensure(host) {
    if (!validHost(host)) throw new Error(`Unknown routines host ${host}`);
    if (host !== LOCAL) getConnections().get(host); // throws for a deleted profile
    const state = stateOf(host);
    if (!state || state.state !== "ready")
      throw new Error(
        state?.message || `The sushiai daemon on ${host} is not ready.`,
      );
    if (hasModule(state)) return;
    if (!moduleSwitch)
      throw new Error(`Routines are not turned on on ${host}.`);
    await moduleSwitch.enable(host);
  }

  async function call(host, method, params = {}) {
    assertOn();
    if (!DAEMON_METHODS.has(method))
      throw new Error(`${method} is not available`);
    await ensure(host);
    let sent = params && typeof params === "object" ? { ...params } : {};
    if (WITH_CATALOG.has(method)) {
      const full = await catalog();
      sent = { ...sent, catalog: daemonCatalog(full, host) };
      // A routine runs on the daemon it is saved to: it names that host the
      // way the daemon's own folders do.
      if (sent.routine && typeof sent.routine === "object") {
        const target = full.hosts.find((item) => item.id === host);
        sent.routine = {
          ...sent.routine,
          where: {
            ...sent.routine.where,
            host: target?.hostName || LOCAL,
          },
        };
      }
    }
    return manager().request(
      managerHostOf(host),
      `routines.${method}`,
      sent,
      method === "dry_run" || method === "save"
        ? { timeoutMs: 60000 }
        : undefined,
    );
  }

  /** Every host that serves the module, with its routines tagged by host. */
  async function list() {
    assertOn();
    const hosts = hostList().filter(
      (host) => host.state === "ready" && host.routines,
    );
    const unreachable = [];
    const answers = await Promise.all(
      hosts.map(async (host) => {
        try {
          const result = await manager().request(
            managerHostOf(host.id),
            "routines.list",
            {},
          );
          return (result?.routines ?? []).map((item) => ({
            ...item,
            host: host.id,
            hostName: host.name,
          }));
        } catch (error) {
          unreachable.push({
            host: host.id,
            name: host.name,
            error: error.message,
          });
          return [];
        }
      }),
    );
    return { routines: answers.flat(), unreachable };
  }

  /** Moves a routine to the daemon of another host: saved there with the same
   * brief (as a draft), turned on again when it was on, then removed here. */
  async function move(from, to, id) {
    if (from === to) return { host: to, id };
    const shown = await call(from, "show", { id });
    await call(to, "save", { routine: shown.routine, brief: shown.brief });
    if (shown.routine.enabled)
      await call(to, "set_enabled", { id, enabled: true });
    await call(from, "delete", { id });
    return { host: to, id };
  }

  /** The private working folder of one build on a host (unique per build, so
   * resume never mixes two builds): under the
   * app data dir here, under ~/.sushiai on an ssh host. Never a shared tmp. */
  async function builderDir(host, build) {
    if (builderCwd) return builderCwd(host);
    if (host === LOCAL) {
      const base = userDataDir || path.join(os.homedir(), ".sushiai");
      const dir = path.join(base, "routines-builder", "local", build);
      fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
      return dir;
    }
    const out = await getConnections().exec(
      managerHostOf(host),
      `d="$HOME/${REMOTE_BUILDER_DIR}/${build}" && mkdir -p "$d" && chmod 700 "$d" && printf %s "$d"`,
    );
    const dir = String(out ?? "").trim();
    if (!dir.startsWith("/"))
      throw new Error(`Could not prepare the builder folder on ${host}.`);
    return dir;
  }

  /** The builder's settings, repaired against what this Mac's daemon says the
   * harness offers. */
  async function settings() {
    return repaired(readStore(SETTINGS_STORE));
  }

  /** `value` repaired against the catalog of the host the builder starts on. */
  async function repaired(value) {
    const host = validHost(value?.host) ? value.host : LOCAL;
    return builderSettings(value, await agents.of(managerHostOf(host)));
  }

  async function startBuilder({ text, routineId } = {}) {
    assertOn();
    if (typeof text !== "string" || !text.trim())
      throw new Error("Describe what the routine should do.");
    if (text.length > 8000) throw new Error("The request is too long.");
    if (routineId !== undefined && typeof routineId !== "string")
      throw new Error("Invalid routine.");
    const config = await settings();
    await ensure(config.host);
    const cwd = await builderDir(config.host, randomUUID());
    const request = {
      host: managerHostOf(config.host),
      agent: config.harness,
      cwd,
      headless: true,
      // Wide, so a one-line JSON answer is not wrapped by the terminal.
      cols: 400,
      rows: 32,
      title: "Routine builder",
      prompt: builderPrompt(text, routineId, config),
      extraArgs: builderArgs(config),
      idempotencyKey: `routine-builder-${randomUUID()}`,
      background: true,
    };
    if (config.model) request.model = config.model;
    if (config.accountId)
      request[`${config.harness}AccountId`] = config.accountId;
    const started = await launch(request);
    const turn = track(config, request, started.sessionId);
    // `sessionHost` is the key the daemon IPC takes for session calls.
    return {
      host: config.host,
      sessionHost: managerHostOf(config.host),
      sessionId: started.sessionId,
      harness: config.harness,
      turn,
    };
  }

  // Turns of the headless builder, by session id. The harness prints its
  // answer and exits, and an exited session cannot be read: the watcher keeps
  // the last screen until the answer parses.
  const turns = new Map();

  function track(config, request, sessionId) {
    const record = {
      harness: config.harness,
      request,
      host: config.host,
      sessionHost: request.host,
      state: "running",
      text: "",
      agentSession: "",
    };
    turns.set(sessionId, record);
    void watch(record, sessionId);
    return view(record);
  }

  const view = ({ state, text, agentSession }) => ({
    state,
    text,
    agentSession,
  });

  function settle(record, answer) {
    if (record.state !== "running") return;
    record.state = answer.state;
    record.text = answer.text;
    record.agentSession = answer.session;
  }

  async function watch(record, sessionId) {
    const harness = harnessFor(record.harness);
    let last = "";
    let failures = 0;
    for (let at = 0; at < turnLimit; at += turnPollMs) {
      try {
        const read = await manager().request(
          record.sessionHost,
          "session.read",
          { id: sessionId, scrollback: 500 },
        );
        if (String(read?.text ?? "").trim()) last = read.text;
        const answer = harness.parseTurn(read?.text);
        if (answer.state !== "running") return settle(record, answer);
        failures = 0;
      } catch (error) {
        if (/not running|not found/i.test(error.message) || ++failures > 8)
          break;
      }
      await new Promise((resolve) => setTimeout(resolve, turnPollMs).unref?.());
    }
    const answer = harness.parseTurn(last);
    if (answer.state !== "running") return settle(record, answer);
    record.state = "failed";
    record.text = last.trim()
      ? last.trim().slice(-600)
      : "The builder stopped without an answer.";
  }

  function turnOf(sessionId) {
    const record = turns.get(sessionId);
    if (!record) throw new Error("This build is no longer available.");
    return record;
  }

  /** The owner's next message: a new print-mode turn that resumes the
   * conversation of the previous one, in the same folder with the same flags. */
  async function replyBuilder({ sessionId, text } = {}) {
    assertOn();
    if (typeof text !== "string" || !text.trim())
      throw new Error("Write a message to the builder.");
    if (text.length > 8000) throw new Error("The message is too long.");
    const previous = turnOf(sessionId);
    if (previous.state === "running")
      throw new Error("The builder is still working.");
    if (!previous.agentSession)
      throw new Error("The builder did not report its conversation.");
    const request = {
      ...previous.request,
      prompt: text.trim(),
      resume: previous.agentSession,
      idempotencyKey: `routine-builder-${randomUUID()}`,
    };
    const started = await launch(request);
    const turn = track({ harness: request.agent }, request, started.sessionId);
    return {
      host: previous.host,
      sessionHost: previous.sessionHost,
      sessionId: started.sessionId,
      agentSession: previous.agentSession,
      turn,
    };
  }

  /** A plain shell session on `host` that runs the harness's login, for a
   * routine whose run failed with a login problem. `host` is an app host id
   * ("local", "ssh:<profile>") or the host name a run recorded as `loginHost`.
   * Returns where the session is, for the renderer to open in a pane. */
  async function openLoginTerminal(input) {
    assertOn();
    const { host: named, harness } =
      input && typeof input === "object" ? input : {};
    if (!isHarness(harness)) throw new Error("Choose claude or codex.");
    const host = validHost(named)
      ? named
      : hostList().find((item) => item.hostName && item.hostName === named)?.id;
    if (!host) throw new Error(`Unknown host ${named}`);
    const state = stateOf(host);
    if (!state || state.state !== "ready")
      throw new Error(
        state?.message || `The sushiai daemon on ${host} is not ready.`,
      );
    let cwd = os.homedir();
    if (host !== LOCAL) {
      const out = await getConnections().exec(
        managerHostOf(host),
        'printf %s "$HOME"',
      );
      cwd = String(out ?? "").trim();
      if (!cwd.startsWith("/"))
        throw new Error(`Could not find the home folder on ${host}.`);
    }
    const started = await manager().request(
      managerHostOf(host),
      "session.create",
      {
        cmd: loginTerminalCommand(harness),
        cwd,
        cols: 120,
        rows: 32,
        title: `Log in: ${harness}`,
        idempotencyKey: `routine-login-${randomUUID()}`,
      },
    );
    const sessionId = started?.id ?? started?.sessionId;
    if (typeof sessionId !== "string" || !sessionId)
      throw new Error("The daemon did not report the new session.");
    return { host, sessionHost: managerHostOf(host), sessionId };
  }

  function listen() {
    if (unsubscribe) return;
    const value = getManager?.();
    if (!value) return;
    unsubscribe = value.on("event", (event) => {
      if (!isEnabled() || event.method !== EVENT) return;
      send("module-event", {
        moduleId: ROUTINES_MANIFEST.id,
        host: hostIdOf(event.host),
        event: event.params,
      });
    });
  }

  /** Hosts whose daemon serves the module now and are connected. */
  function servingHosts() {
    return hostList()
      .filter((host) => host.state === "ready" && host.routines)
      .map((host) => host.id);
  }

  function retiredHosts() {
    const value = readStore?.(RETIRED_STORE);
    return Array.isArray(value?.hosts)
      ? value.hosts.filter((host) => typeof host === "string")
      : [];
  }

  function setRetired(hosts) {
    writeStore?.(RETIRED_STORE, { hosts: [...new Set(hosts)].sort() });
  }

  /** Serves the module again on a host that was retired, when it is ready. */
  async function restore(host) {
    if (!retiredHosts().includes(host) || !moduleSwitch) return;
    const state = stateOf(host);
    if (!state || state.state !== "ready") return;
    try {
      if (!hasModule(state)) await moduleSwitch.enable(host);
      setRetired(retiredHosts().filter((item) => item !== host));
    } catch (error) {
      log(`routines enable on ${host}: ${error.message}`);
    }
  }

  /** Turning the extension on: listen, serve the module on This Mac and on
   * every host that served it before Routines was turned off. */
  async function enable() {
    listen();
    if (!moduleSwitch) return;
    try {
      await moduleSwitch.enable(LOCAL);
    } catch (error) {
      log(`routines enable on ${LOCAL}: ${error.message}`);
    }
    await Promise.all(retiredHosts().map(restore));
  }

  /** Turning it off: every serving host forgets the schedules (the store stays
   * on disk), then the module is unregistered there. */
  async function disable() {
    const hosts = servingHosts();
    setRetired([...retiredHosts(), ...hosts.filter((host) => host !== LOCAL)]);
    unsubscribe?.();
    unsubscribe = null;
    await Promise.all(hosts.map(retire));
  }

  /** Retires the module on a host that still serves it while Routines is off
   * (a host that was offline when the owner turned it off). */
  async function retire(host) {
    try {
      await manager().request(managerHostOf(host), "routines.suspend", {});
    } catch (error) {
      log(`routines suspend on ${host}: ${error.message}`);
    }
    try {
      await moduleSwitch?.disable(host);
    } catch (error) {
      log(`routines disable on ${host}: ${error.message}`);
    }
  }

  const retiring = new Set();
  async function sweepOff() {
    if (isEnabled()) return;
    for (const host of servingHosts()) {
      if (retiring.has(host)) continue;
      if (host !== LOCAL) setRetired([...retiredHosts(), host]);
      retiring.add(host);
      try {
        await retire(host);
      } finally {
        retiring.delete(host);
      }
    }
  }

  /** While Routines is off, a host that turns ready and still serves the
   * module is retired too. Returns the unsubscribe. */
  function watchHosts() {
    const value = getManager?.();
    if (!value) return () => {};
    const off = value.on("state", (state) => {
      if (state?.state !== "ready") return;
      if (isEnabled()) restore(hostIdOf(state.host)).catch(() => {});
      else sweepOff().catch(() => {});
    });
    sweepOff().catch(() => {});
    return off;
  }

  const methods = {
    catalog: () => (assertOn(), catalog()),
    list,
    show: (host, id) => call(host, "show", { id }),
    runs: (host, id) => call(host, "runs", { id }),
    result: (host, id, run) =>
      call(host, "result", { id, ...(run ? { run } : {}) }),
    validate: (host, payload) => call(host, "validate", payload),
    save: (host, payload) => call(host, "save", payload),
    setEnabled: (host, id, enabled) =>
      call(host, "set_enabled", { id, enabled: enabled === true }),
    runNow: (host, id) => call(host, "run_now", { id }),
    dryRun: (host, id) => call(host, "dry_run", { id }),
    delete: (host, id) => call(host, "delete", { id }),
    move: (from, to, id) => (assertOn(), move(from, to, id)),
    "settings.get": () => settings(),
    "settings.set": async (patch) => {
      if (!patch || typeof patch !== "object" || Array.isArray(patch))
        throw new Error("Invalid settings.");
      const current = await settings();
      const next = await repaired({
        ...current,
        ...patch,
        // `sees` is patched key by key: {sees: {projects: false}} keeps the other.
        sees: { ...current.sees, ...(patch.sees ?? {}) },
      });
      writeStore(SETTINGS_STORE, next);
      return next;
    },
    "builder.start": startBuilder,
    "builder.reply": replyBuilder,
    "builder.turn": ({ sessionId } = {}) => view(turnOf(sessionId)),
    "open-login-terminal": openLoginTerminal,
    grant: (host, id, tools) =>
      call(host, "grant", {
        id,
        ...(tools === "all" ? { all: true } : { tools }),
      }),
    revoke: (host, id, tools) => call(host, "revoke", { id, tools }),
    approve: (host, id, run, proposal) =>
      call(host, "approve", { id, run, proposal }),
  };

  return {
    methods,
    call,
    list,
    catalog,
    enable,
    disable,
    listen,
    ensure,
    watchHosts,
  };
}

/** Wires the module into the app: module IPC, the extension's on/off, the
 * event forwarding. `extensions` is the ExtensionManager. */
function registerRoutines({ moduleIpc, extensions, ...options }) {
  const routines = createRoutines({
    ...options,
    isEnabled: () => extensions.isEnabled(ROUTINES_MANIFEST.id),
  });
  moduleIpc.register(ROUTINES_MANIFEST.id, routines.methods);
  let started = false;
  const apply = () =>
    (extensions.isEnabled(ROUTINES_MANIFEST.id)
      ? routines.enable()
      : routines.disable()
    ).catch((error) => options.log?.(`routines: ${error.message}`));
  extensions.onChange((id) => {
    if (started && id === ROUTINES_MANIFEST.id) return apply();
  });
  routines.start = async () => {
    await extensions.ready;
    started = true;
    if (extensions.isEnabled(ROUTINES_MANIFEST.id)) routines.listen();
    routines.watchHosts();
  };
  return routines;
}

module.exports = {
  createRoutines,
  registerRoutines,
  builderSettings,
  builderArgs,
  builderPrompt,
  loginTerminalCommand,
  ROUTINES_SKILL_NAME: SKILL,
};
