// The `sushiai` daemon the desktop scripts start: the newest of target/release
// and target/debug, built (release) when neither exists. Scripts pass the
// result as SUSHIAI_DAEMON_BIN, with their own SUSHIAI_HOME, so they never
// touch the owner's ~/.sushiai daemon.
import { existsSync, readFileSync, statSync } from "node:fs";
import { execFileSync } from "node:child_process";
import path from "node:path";

export function daemonBinary(root = process.cwd()) {
  const found = () =>
    ["release", "debug"]
      .map((profile) => path.join(root, "target", profile, "sushiai"))
      .filter((file) => existsSync(file))
      .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
  if (!found())
    execFileSync("cargo", ["build", "--release", "-p", "sushiai"], {
      cwd: root,
      stdio: "inherit",
    });
  return found();
}

/** Stops the daemon of a script's own SUSHIAI_HOME and every holder under it.
 * The daemon outlives the app by design, so a script that started one ends it. */
export function stopDaemon(home) {
  const lines = execFileSync("ps", ["-axo", "pid=,command="], {
    encoding: "utf8",
  }).split("\n");
  const own = lines
    .map((line) => line.trim().match(/^(\d+)\s+(.*)$/))
    .filter((match) => match && match[2].includes(`${home}/`));
  let daemon = null;
  try {
    daemon = Number(
      readFileSync(path.join(home, "daemon.lock"), "utf8").trim(),
    );
  } catch {
    // no daemon was started
  }
  for (const pid of [
    ...own.map((match) => Number(match[1])),
    ...(Number.isInteger(daemon) && daemon > 1 ? [daemon] : []),
  ])
    try {
      process.kill(pid, "SIGKILL");
    } catch {
      // already gone
    }
}
