"use strict";

// What a host offers a routine's agent: whether each harness CLI is installed
// and logged in. One read per host; the caller caches it. This Mac runs the
// probe in a shell, an ssh host through the connection's exec path.
const childProcess = require("node:child_process");
const { PREFLIGHT_SCRIPT, parsePreflight } = require("./host-setup.cjs");
const { posixCommand } = require("./host-install.cjs");

const PROBE_TIMEOUT_MS = 40000;

function runLocal(script, timeout, run) {
  return new Promise((resolve) => {
    run(
      "/bin/sh",
      ["-c", script],
      { timeout, encoding: "utf8" },
      (error, stdout) => resolve(error ? "" : String(stdout)),
    );
  });
}

/** `exec(hostId, command, {timeout})` is the connections' ssh exec; `run` is
 * `execFile` (replaceable in tests). */
function createHostProbe({ exec, run = childProcess.execFile }) {
  return async function probe(hostId) {
    const local = hostId === "local";
    const read = (script) =>
      local
        ? runLocal(script, PROBE_TIMEOUT_MS, run)
        : exec(hostId, posixCommand(script), { timeout: PROBE_TIMEOUT_MS });
    const preflight = parsePreflight(await read(PREFLIGHT_SCRIPT));
    return {
      claude: preflight.claude,
      codex: preflight.codex,
    };
  };
}

module.exports = { createHostProbe };
