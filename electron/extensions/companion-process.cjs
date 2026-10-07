"use strict";

// Companion processes: a native program an extension manifest names (the
// `companion` block). The app starts it only when the extension is enabled,
// the owner approved this exact resolved path + args + permissions, and the
// app is ready. It speaks the daemon frame protocol over stdio, so the client
// side is spawnPipe + connectOverPipe from electron/daemon/connectors.cjs.
//
// App -> companion: hello, view.read {surfaceId}, <action.method> {surfaceId,
// hosts?}. Companion -> app: the notification view.changed {surfaceId} only.
// There are no companion -> app requests. Values are never stored or logged.
// Approval is consent, not a sandbox: the process keeps the owner's rights.

const fs = require("node:fs");
const path = require("node:path");
const {
  spawnPipe,
  connectOverPipe,
  failure,
  tail,
} = require("../daemon/connectors.cjs");
const { resolveHome } = require("../daemon/local.cjs");
const { CONTRACT } = require("./manifest.cjs");

// Only these reach the child, plus SUSHIAI_HOME.
const ENV_ALLOWLIST = [
  "PATH",
  "HOME",
  "USER",
  "LANG",
  "TMPDIR",
  "SUSHIAI_HOME",
];
const BACKOFF_MS = [1000, 5000, 30000];
const WINDOW_MS = 10 * 60 * 1000;
const MAX_EXITS = 3;
const KILL_GRACE_MS = 3000;
const HELLO_TIMEOUT_MS = 10000;
const READ_TIMEOUT_MS = 10000;
const ACTION_TIMEOUT_MS = 30000;
const MAX_TEXT = 1000;
const MAX_QR = 2048;
const MAX_MESSAGE = 500;
const CLIENT_NAME = "sushiai-desktop";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function isFile(candidate) {
  try {
    fs.accessSync(candidate, fs.constants.X_OK);
    return fs.statSync(candidate).isFile();
  } catch {
    return false;
  }
}

const inside = (child, parent) => {
  const relative = path.relative(parent, child);
  return (
    relative === "" ||
    (!relative.startsWith("..") && !path.isAbsolute(relative))
  );
};

/** $SUSHIAI_HOME/bin first, then PATH. Returns {path} or {error}. A command
 * whose real location is inside the extension folder is refused: that folder
 * carries no code. */
function resolveCommand(command, { home, pathEnv, extensionDir }) {
  const dirs = [path.join(home, "bin"), ...String(pathEnv || "").split(":")];
  for (const dir of dirs) {
    if (!dir || !path.isAbsolute(dir)) continue;
    const candidate = path.join(dir, command);
    if (!isFile(candidate)) continue;
    if (extensionDir) {
      try {
        if (inside(fs.realpathSync(candidate), fs.realpathSync(extensionDir)))
          return {
            error: `The command "${command}" is inside the extension folder; an extension folder cannot carry code.`,
          };
      } catch {
        return { error: `Cannot check where "${command}" lives.` };
      }
    }
    return { path: candidate };
  }
  return { error: `The command "${command}" was not found.` };
}

const sameList = (a, b) =>
  Array.isArray(a) && a.length === b.length && a.every((v, i) => v === b[i]);

/** The approval matches only the exact resolved path, args and permissions. */
function approvalMatches(approved, config) {
  return Boolean(
    approved &&
    typeof approved === "object" &&
    approved.path === config.resolvedPath &&
    sameList(approved.args, config.args) &&
    Array.isArray(approved.permissions) &&
    sameList([...approved.permissions].sort(), [...config.permissions].sort()),
  );
}

/** Why a companion ended, for the status line. */
function describeExit({ code, signal, error } = {}) {
  if (error?.code === "ENOENT")
    return `Cannot run the companion: ${error.message}`;
  if (error) return error.message;
  return signal
    ? `The companion ended with ${signal}.`
    : `The companion exited with ${code}.`;
}

function childEnv(source, home) {
  const out = {};
  for (const key of ENV_ALLOWLIST) if (source[key]) out[key] = source[key];
  out.SUSHIAI_HOME = home;
  return out;
}

/** Checks a companion's reply against the view that asked for it. A value the
 * view did not declare, or of the wrong shape, rejects the whole reply. */
function checkResult(result, view) {
  if (result === null || result === undefined) return {};
  if (typeof result !== "object" || Array.isArray(result))
    throw new Error("The companion replied with something unreadable.");
  const out = {};
  if (result.message !== undefined) {
    if (
      typeof result.message !== "string" ||
      result.message.length > MAX_MESSAGE
    )
      throw new Error("The companion sent an invalid message.");
    out.message = result.message;
  }
  if (result.values !== undefined) {
    if (
      !result.values ||
      typeof result.values !== "object" ||
      Array.isArray(result.values)
    )
      throw new Error("The companion sent invalid values.");
    const fields = new Map((view?.fields || []).map((f) => [f.id, f]));
    out.values = {};
    for (const [key, value] of Object.entries(result.values)) {
      const field = fields.get(key);
      if (!field)
        throw new Error(`The companion sent an unknown field: ${key}.`);
      let ok = value === null;
      if (field.type === "text")
        ok = ok || (typeof value === "string" && value.length <= MAX_TEXT);
      if (field.type === "qr")
        ok = ok || (typeof value === "string" && value.length <= MAX_QR);
      if (field.type === "status")
        ok =
          ok ||
          (typeof value?.text === "string" &&
            value.text.length <= MAX_TEXT &&
            CONTRACT.TONES.has(value.tone));
      if (!ok) throw new Error(`The companion sent a bad value for ${key}.`);
      out.values[key] =
        value !== null && field.type === "status"
          ? { text: value.text, tone: value.tone }
          : value;
    }
  }
  return out;
}

function createCompanions({
  getHosts = () => [],
  home,
  env = process.env,
  spawnProcess = spawnPipe,
  backoffMs = BACKOFF_MS,
  windowMs = WINDOW_MS,
  maxExits = MAX_EXITS,
  killGraceMs = KILL_GRACE_MS,
  helloTimeoutMs = HELLO_TIMEOUT_MS,
  readTimeoutMs = READ_TIMEOUT_MS,
  actionTimeoutMs = ACTION_TIMEOUT_MS,
} = {}) {
  const base = home || resolveHome(env);
  const entries = new Map();
  const listeners = new Set();
  let started = false;
  // Set by stopAll (quit): nothing may spawn afterwards, even if start() or
  // sync() was still waiting on startup work.
  let stopped = false;

  const emit = (event) => {
    for (const listener of listeners) {
      try {
        listener(event);
      } catch {
        // A throwing listener must not break supervision.
      }
    }
  };
  const setState = (entry, state, stderrTail) => {
    entry.state = state;
    if (stderrTail !== undefined) entry.stderrTail = stderrTail;
    emit({ type: "status", extensionId: entry.id });
  };
  const config = (entry) => ({
    resolvedPath: entry.resolvedPath,
    args: entry.args,
    permissions: entry.permissions,
  });
  const idle = (entry) => !entry.pipe && !entry.timer;

  async function terminate(pipe) {
    const child = pipe?.child;
    if (!child || pipe.exit) return;
    child.kill("SIGTERM");
    const done = pipe.exited.then(() => true);
    if (await Promise.race([done, sleep(killGraceMs).then(() => false)]))
      return;
    child.kill("SIGKILL");
    await done;
  }

  async function launch(entry) {
    if (!entry.wanted()) return;
    const run = ++entry.run;
    setState(entry, "starting");
    const pipe = spawnProcess(entry.resolvedPath, entry.args, {
      env: childEnv(env, base),
    });
    entry.pipe = pipe;
    let client;
    try {
      client = await connectOverPipe({
        pipe,
        clientName: CLIENT_NAME,
        helloTimeoutMs,
        classify: (exit) => failure(describeExit(exit), { exit }),
      });
    } catch (error) {
      void exited(entry, run, error.exit || pipe.exit, error);
      return;
    }
    if (entry.run !== run) {
      client.close();
      return;
    }
    entry.client = client;
    client.on("view.changed", (params) => {
      if (typeof params?.surfaceId === "string")
        emit({
          type: "changed",
          extensionId: entry.id,
          surfaceId: params.surfaceId,
        });
    });
    client.on("disconnect", () => void exited(entry, run, pipe.exit, null));
    setState(entry, "running", "");
  }

  async function exited(entry, run, exit, error) {
    if (entry.run !== run) return;
    entry.client = null;
    // The process may outlive its pipe (a bad hello, a malformed frame). Keep
    // the pipe on the entry until the process is gone, so halt() can still
    // reach it and a retry never stacks a second process on a live one.
    const { pipe } = entry;
    await terminate(pipe);
    if (entry.run !== run) return;
    entry.pipe = null;
    exit = exit || pipe?.exit;
    const now = Date.now();
    entry.exits = entry.exits.filter((at) => now - at < windowMs);
    entry.exits.push(now);
    const detail =
      tail(exit?.stderr) ||
      describeExit({ ...exit, error: error ?? exit?.error });
    if (entry.exits.length >= maxExits) {
      setState(entry, "failed", detail);
      return;
    }
    setState(entry, "starting", detail);
    const wait =
      backoffMs[Math.min(entry.exits.length - 1, backoffMs.length - 1)];
    entry.timer = setTimeout(() => {
      entry.timer = null;
      launch(entry);
    }, wait);
    entry.timer.unref?.();
  }

  /** Ends the process and any pending restart; the entry reads as off. */
  async function halt(entry) {
    entry.run += 1;
    clearTimeout(entry.timer);
    entry.timer = null;
    const { pipe, client } = entry;
    entry.pipe = null;
    entry.client = null;
    entry.state = "off";
    await terminate(pipe);
    client?.close();
  }

  async function remove(extensionId) {
    const entry = entries.get(extensionId);
    if (!entry) return;
    await halt(entry);
    entries.delete(extensionId);
  }

  /** Brings one extension's process in line with what it should be doing now.
   * Resolves once a process that has to stop is gone. */
  async function sync(
    extensionId,
    { manifest, enabled, approved, extensionDir },
  ) {
    if (stopped) return;
    const block = manifest?.companion;
    if (!block) return remove(extensionId);
    let entry = entries.get(extensionId);
    if (!entry) {
      entry = {
        id: extensionId,
        state: "off",
        run: 0,
        exits: [],
        timer: null,
        pipe: null,
        client: null,
        stderrTail: "",
        resolvedPath: undefined,
        args: [],
        permissions: [],
        wanted: () => false,
      };
      entries.set(extensionId, entry);
    }
    const resolved = resolveCommand(block.command, {
      home: base,
      pathEnv: env.PATH,
      extensionDir,
    });
    const before = config(entry);
    entry.resolvedPath = resolved.path;
    entry.args = [...block.args];
    entry.permissions = [...block.permissions];
    const changed =
      before.resolvedPath !== entry.resolvedPath ||
      !sameList(before.args, entry.args) ||
      !sameList(before.permissions, entry.permissions);
    const wasWanted = entry.wanted();
    const approvedNow =
      Boolean(resolved.path) && approvalMatches(approved, config(entry));
    const wants = Boolean(enabled) && approvedNow;
    entry.wanted = () => wants;
    // A switch off or a different command ends the process. A failed entry
    // stays failed until one of those or a new approval clears it.
    if (!wants || changed || !wasWanted) {
      if (!idle(entry) || entry.state === "failed") await halt(entry);
      entry.exits = [];
    }
    if (!wants) {
      const state = !enabled
        ? "off"
        : resolved.error
          ? "failed"
          : "needs-approval";
      if (entry.state !== state || entry.stderrTail !== (resolved.error ?? ""))
        setState(entry, state, resolved.error ?? "");
      return;
    }
    if (started && idle(entry) && entry.state !== "failed") await launch(entry);
  }

  function running(extensionId) {
    const entry = entries.get(extensionId);
    if (!entry || entry.state !== "running" || !entry.client)
      throw new Error("The companion is not running.");
    return entry;
  }

  return {
    sync,
    remove,
    /** The app is ready: processes that are wanted may start. */
    async start() {
      if (stopped) return;
      started = true;
      for (const entry of entries.values())
        if (entry.wanted() && idle(entry) && entry.state !== "failed")
          await launch(entry);
    },
    /** Stops every process (quit). */
    async stopAll() {
      stopped = true;
      started = false;
      await Promise.all([...entries.values()].map((entry) => halt(entry)));
    },
    /** What an approval would bind: the resolved path, args and permissions. */
    describe(extensionId) {
      const entry = entries.get(extensionId);
      return entry ? config(entry) : undefined;
    },
    status(extensionId) {
      const entry = entries.get(extensionId);
      if (!entry) return undefined;
      return {
        state: entry.state,
        ...(entry.resolvedPath ? { resolvedPath: entry.resolvedPath } : {}),
        args: [...entry.args],
        permissions: [...entry.permissions],
        ...(entry.stderrTail ? { stderrTail: entry.stderrTail } : {}),
      };
    },
    hosts: () => getHosts(),
    async read(extensionId, surfaceId, view) {
      const result = await running(extensionId).client.request(
        "view.read",
        { surfaceId },
        { timeoutMs: readTimeoutMs },
      );
      return checkResult(result, view);
    },
    async call(extensionId, method, params, view) {
      const result = await running(extensionId).client.request(method, params, {
        timeoutMs: actionTimeoutMs,
      });
      return checkResult(result, view);
    },
    /** listener({type:"changed", extensionId, surfaceId}) when the companion
     * says a view changed; listener({type:"status", extensionId}) when its
     * state moves. Returns an unsubscribe. */
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

module.exports = {
  createCompanions,
  resolveCommand,
  approvalMatches,
  checkResult,
  describeExit,
  ENV_ALLOWLIST,
};
