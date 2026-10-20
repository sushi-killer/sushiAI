// One generous deadline for tests that wait on a real process, plus ordered
// cleanup helpers. Tests wait for real events against this deadline instead of
// raising per-wait numbers, so they pass on a loaded machine.
const fs = require("node:fs");
const { spawnSync } = require("node:child_process");
const { resolveBinary } = require("../../electron/daemon/local.cjs");

const DEFAULT_DEADLINE_MS = 180000;
const POLL_MS = 25;

/** Deadline in ms: `SUSHIAI_TEST_DEADLINE_SECS` (a positive number) or 180 s. */
function deadlineMs(env = process.env) {
  const secs = Number(env.SUSHIAI_TEST_DEADLINE_SECS);
  return Number.isFinite(secs) && secs > 0 ? secs * 1000 : DEFAULT_DEADLINE_MS;
}

const DEADLINE_MS = deadlineMs();

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Polls `check` until it returns a truthy value; throws after the deadline. */
async function until(check, what, { deadlineMs: limit = DEADLINE_MS } = {}) {
  const end = Date.now() + limit;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() >= end) throw new Error(`timed out waiting for ${what}`);
    await delay(POLL_MS);
  }
}

/** One event-loop turn: every callback already queued has run. */
const turn = () => new Promise((resolve) => setImmediate(resolve));

/** Like `until`, counted in event-loop turns instead of time, for tests that
 * mock setTimeout (the `until` poll would never fire). Throws after `turns`. */
async function untilTurns(check, what, { turns = 1000 } = {}) {
  for (let i = 0; i < turns; i++) {
    const value = await check();
    if (value) return value;
    await turn();
  }
  throw new Error(`no ${what} after ${turns} event-loop turns`);
}

function isAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error.code === "ESRCH") return false;
    return true;
  }
}

/** The native start time of a live pid, or "" when it is gone. Taken when a
 * pid is first seen and passed to `stopPid` as `start`, it keeps a cleanup from
 * signalling an unrelated process that reused the pid. */
function pidStart(pid) {
  const result = spawnSync(resolveBinary(), ["proc", String(pid)], {
    encoding: "utf8",
  });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(result.stderr || "process probe failed");
  const info = JSON.parse(result.stdout);
  return info && !info.zombie ? info.startTime : "";
}

async function waitGone(pid, limit) {
  const end = Date.now() + limit;
  while (isAlive(pid)) {
    if (Date.now() >= end) return false;
    await delay(POLL_MS);
  }
  return true;
}

/** Ends a process (negative pid: a process group) and waits until it is gone.
 * SIGTERM first, SIGKILL after the deadline; throws if it still lives. With `start`
 * (from `pidStart`), a pid whose start time differs is a reused pid and is left alone. */
async function stopPid(pid, { deadlineMs: limit = DEADLINE_MS, start } = {}) {
  if (!pid || !isAlive(pid)) return;
  if (start !== undefined && pidStart(pid) !== start) return;
  const send = (signal) => {
    try {
      process.kill(pid, signal);
    } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
  };
  send("SIGTERM");
  if (await waitGone(pid, limit)) return;
  send("SIGKILL");
  if (await waitGone(pid, limit)) return;
  throw new Error(`pid ${pid} is still alive after SIGKILL`);
}

/** Recursive removal that retries while a dying process still writes. */
function removeDir(dir) {
  fs.rmSync(dir, {
    recursive: true,
    force: true,
    maxRetries: 40,
    retryDelay: 100,
  });
}

module.exports = {
  DEADLINE_MS,
  deadlineMs,
  until,
  turn,
  untilTurns,
  pidStart,
  stopPid,
  removeDir,
};
