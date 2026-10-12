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
  let pid;
  try {
    pid = Number(readFileSync(path.join(home, "daemon.lock"), "utf8").trim());
  } catch (error) {
    if (error.code === "ENOENT") return;
    throw error;
  }
  if (!Number.isInteger(pid) || pid <= 1) return;
  const binary = path.join(home, "bin", "sushiai");
  const probe = () =>
    JSON.parse(
      execFileSync(binary, ["proc", String(pid)], { encoding: "utf8" }),
    );
  const started = probe();
  if (!started || started.zombie) return;
  execFileSync(binary, ["shutdown"], {
    env: { ...process.env, HOME: path.dirname(home), SUSHIAI_HOME: home },
    timeout: 30000,
    stdio: "pipe",
  });
  const deadline = Date.now() + 15000;
  for (;;) {
    const current = probe();
    if (!current || current.zombie || current.startTime !== started.startTime)
      return;
    if (Date.now() >= deadline)
      throw new Error(`test daemon ${pid} did not stop`);
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 25);
  }
}
