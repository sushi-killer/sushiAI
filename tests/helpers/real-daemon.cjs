// One lookup for the built `sushiai` binary that the real-daemon tests drive.
// Locally a missing binary skips those tests. Under CI it fails the file
// instead, so a runner that forgot to build cannot stay green by skipping.
const fs = require("node:fs");
const path = require("node:path");

function findBinary(exists) {
  const targets = [
    process.env.CARGO_TARGET_DIR,
    path.join(__dirname, "..", "..", "target"),
  ].filter(Boolean);
  return [
    process.env.SUSHIAI_DAEMON_BIN,
    ...targets.map((dir) => path.join(dir, "debug", "sushiai")),
    ...targets.map((dir) => path.join(dir, "release", "sushiai")),
  ].find((file) => file && exists(file));
}

/** `{ binary, skip }`: `skip` is false when the binary exists, a reason string
 * when it does not and the run is local. Throws under CI. */
function realDaemon(env = process.env, exists = fs.existsSync) {
  const binary = findBinary(exists);
  if (binary) return { binary, skip: false };
  const reason = "sushiai binary is not built (run: cargo build -p sushiai)";
  if (env.CI) throw new Error(`${reason}; CI must not skip real-daemon tests`);
  return { binary: undefined, skip: reason };
}

module.exports = { realDaemon };
