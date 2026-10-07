"use strict";

// `host.exec`: a companion with the permission `hosts.exec` asks the app to run
// a command on one of the owner's saved ssh hosts. The app runs it over its own
// ssh path, after the owner allowed THIS call in a card that shows the exact
// argv. There is no standing allowance. argv, stdin and output are never
// logged or stored; the audit line holds only their SHA-256 hashes.

const { createHash } = require("node:crypto");
const { listSshHosts } = require("./hosts.cjs");

const MAX_TITLE = 120;
const MAX_ARGV_WORDS = 64;
const MAX_ARGV_CHARS = 256 * 1024;
const MAX_STDIN_BYTES = 32 * 1024 * 1024;
const MAX_TIMEOUT_MS = 300000;
const DEFAULT_TIMEOUT_MS = 120000;
const TAIL_BYTES = 64 * 1024;
const ASK_TIMEOUT_MS = 2 * 60 * 1000;
const MAX_TOTAL = 4;
const BASE64 = /^[A-Za-z0-9+/]*={0,2}$/;

// JSON-RPC style codes the companion can tell apart.
const CODES = {
  failed: -32000,
  refused: -32001,
  busy: -32002,
  permission: -32003,
  host: -32004,
  params: -32602,
};

const fail = (name, message) =>
  Object.assign(new Error(message), { code: CODES[name] });

const tail = (text) => {
  const bytes = Buffer.from(String(text ?? ""), "utf8");
  return bytes.length > TAIL_BYTES
    ? bytes.subarray(-TAIL_BYTES).toString("utf8")
    : bytes.toString("utf8");
};

/** Checks the request's shape; returns the clean values. */
function checkParams(params) {
  if (!params || typeof params !== "object" || Array.isArray(params))
    throw fail("params", "host.exec needs an object.");
  const { host, title, argv, stdin, timeoutMs } = params;
  if (typeof host !== "string" || !host)
    throw fail("params", "host.exec needs a host id.");
  if (
    typeof title !== "string" ||
    !title.trim() ||
    title.length > MAX_TITLE ||
    // eslint-disable-next-line no-control-regex
    /[\u0000-\u001f\u007f]/.test(title)
  )
    throw fail("params", `title is 1 to ${MAX_TITLE} characters, one line.`);
  if (
    !Array.isArray(argv) ||
    !argv.length ||
    argv.length > MAX_ARGV_WORDS ||
    argv.some((word) => typeof word !== "string" || word.includes("\0")) ||
    argv.reduce((sum, word) => sum + word.length, 0) > MAX_ARGV_CHARS
  )
    throw fail("params", "argv is a list of strings.");
  let input = Buffer.alloc(0);
  if (stdin !== undefined && stdin !== null) {
    if (
      typeof stdin !== "string" ||
      stdin.length > Math.ceil(MAX_STDIN_BYTES / 3) * 4 ||
      stdin.length % 4 !== 0 ||
      !BASE64.test(stdin)
    )
      throw fail("params", "stdin is base64 of at most 32 MiB.");
    input = Buffer.from(stdin, "base64");
    if (input.length > MAX_STDIN_BYTES)
      throw fail("params", "stdin is base64 of at most 32 MiB.");
  }
  if (
    timeoutMs !== undefined &&
    (!Number.isInteger(timeoutMs) ||
      timeoutMs < 1 ||
      timeoutMs > MAX_TIMEOUT_MS)
  )
    throw fail("params", `timeoutMs is at most ${MAX_TIMEOUT_MS}.`);
  return {
    host,
    title,
    argv,
    input,
    timeoutMs: timeoutMs ?? DEFAULT_TIMEOUT_MS,
  };
}

const sha256 = (data) => createHash("sha256").update(data).digest("hex");

/**
 * `getProfiles()` the saved connection profiles; `execOnHost(endpoint, argv,
 * {input, timeout, tailBytes})` resolves {code, stdout, stderr};
 * `askOwner(question)` resolves true (Allow) or false (Deny); `audit(line)`
 * receives the audit record. The question holds everything the card shows:
 * extension id and name, host id, name and address, the exact argv, the
 * extension's own description, and the size and SHA-256 of stdin.
 */
function createHostExec({
  getProfiles,
  execOnHost,
  askOwner,
  audit = () => {},
  now = Date.now,
  askTimeoutMs = ASK_TIMEOUT_MS,
}) {
  const busyHosts = new Set();
  // One card at a time per companion: each call waits for the one before it.
  const turns = new Map(); // extension -> tail of its card queue
  const restarts = new Map(); // extension -> how often it restarted

  const record = (line) => {
    try {
      audit({ ...line, at: now() });
    } catch {
      // An audit sink must not change the result.
    }
  };

  /** Shows one card; resolves "allowed", "denied" or "timeout". The 2 minutes
   * start when the card is shown, not while it waits in line. */
  function ask(extensionId, question) {
    const previous = turns.get(extensionId) ?? Promise.resolve();
    const mine = previous.then(
      () =>
        new Promise((resolve) => {
          const timer = setTimeout(resolve, askTimeoutMs, "timeout");
          Promise.resolve()
            .then(() => askOwner(question))
            .then(
              (answer) => answer === true,
              () => false,
            )
            .then((yes) => {
              clearTimeout(timer);
              resolve(yes ? "allowed" : "denied");
            });
        }),
    );
    turns.set(
      extensionId,
      mine.then(() => {}),
    );
    return mine;
  }

  const full = (hostId) => {
    if (busyHosts.has(hostId))
      throw fail("busy", "Another command is running on this host.");
    if (busyHosts.size >= MAX_TOTAL)
      throw fail("busy", "Too many commands are running.");
  };

  /** `companion` is {id, name, permissions}. */
  async function run(companion, params) {
    if (!companion.permissions.includes("hosts.exec"))
      throw fail("permission", "This companion has no hosts.exec permission.");
    const request = checkParams(params);
    if (request.host === "local")
      throw fail("host", "Commands do not run on this computer.");
    const profile = listSshHosts(getProfiles).find(
      (item) => item.id === request.host,
    );
    if (!profile) throw fail("host", "Unknown host.");
    full(profile.id);
    const hashes = {
      extensionId: companion.id,
      hostId: profile.id,
      argvSha256: sha256(JSON.stringify(request.argv)),
      stdinSha256: request.input.length ? sha256(request.input) : null,
    };
    const generation = restarts.get(companion.id) ?? 0;
    const decision = await ask(companion.id, {
      extensionId: companion.id,
      extensionName: companion.name,
      hostId: profile.id,
      hostName: profile.name,
      hostAddress: `${profile.host}${profile.port ? `:${profile.port}` : ""}`,
      argv: request.argv,
      title: request.title,
      stdinBytes: request.input.length,
      stdinSha256: hashes.stdinSha256,
    });
    // An answer that arrives after the companion restarted is not for it.
    const restarted = (restarts.get(companion.id) ?? 0) !== generation;
    if (decision !== "allowed" || restarted) {
      record({
        ...hashes,
        decision: restarted ? "denied" : decision,
        code: null,
      });
      throw fail("refused", "The owner refused");
    }
    full(profile.id);
    busyHosts.add(profile.id);
    try {
      const result = await execOnHost(`ssh:${profile.id}`, request.argv, {
        input: request.input,
        timeout: request.timeoutMs,
        tailBytes: TAIL_BYTES,
      });
      const code = Number.isInteger(result?.code) ? result.code : null;
      record({ ...hashes, decision: "allowed", code });
      return {
        code,
        stdout: tail(result?.stdout),
        stderr: tail(result?.stderr),
      };
    } catch (error) {
      record({ ...hashes, decision: "allowed", code: null });
      throw fail("failed", String(error?.message || error).slice(0, 500));
    } finally {
      busyHosts.delete(profile.id);
    }
  }

  return {
    run,
    /** The companion restarted or stopped: answers still pending are void. */
    forget(extensionId) {
      restarts.set(extensionId, (restarts.get(extensionId) ?? 0) + 1);
    },
  };
}

module.exports = { createHostExec, CODES };
