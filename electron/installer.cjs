const fs = require("node:fs/promises");
const path = require("node:path");
const { spawn } = require("node:child_process");
const { cleanupCompleted } = require("./install-helper.cjs");
function applicationPath(executable = process.execPath) {
  return path.resolve(executable, "../../..");
}
async function install({ directory, release, dmg, packaged, onReady }) {
  if (process.platform !== "darwin" || !packaged)
    throw Error(
      "Automatic installation is available in the packaged macOS app.",
    );
  const planDirectory = await fs.mkdtemp(path.join(directory, "install-"));
  const helper = path.join(planDirectory, "helper.cjs");
  const planFile = path.join(planDirectory, "plan.json");
  await fs.writeFile(
    helper,
    await fs.readFile(path.join(__dirname, "install-helper.cjs")),
    { mode: 0o600 },
  );
  await fs.writeFile(
    planFile,
    JSON.stringify({
      target: await fs.realpath(applicationPath()),
      dmg,
      version: release.version,
      sha256: release.sha256,
      pid: process.pid,
      receipt: path.join(directory, "installed.json"),
      errorFile: path.join(directory, "install-error.txt"),
    }),
    { mode: 0o600 },
  );
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [helper, planFile], {
      detached: true,
      env: { ...process.env, ELECTRON_RUN_AS_NODE: "1" },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let output = "",
      error = "",
      ready = false;
    child.stderr.on("data", (chunk) => {
      error = (error + chunk).slice(-4000);
    });
    child.stdout.on("data", (chunk) => {
      output += chunk;
      if (!ready && output.includes("READY\n")) {
        ready = true;
        child.stdout.unref();
        child.stderr.unref();
        child.unref();
        resolve();
        setImmediate(onReady);
      }
    });
    child.on("error", reject);
    child.on("exit", (code) => {
      fs.rm(planDirectory, { recursive: true, force: true }).catch(() => {});
      if (!ready)
        reject(Error(error.trim() || `Update preparation failed (${code}).`));
    });
  });
}
module.exports = { install, applicationPath, cleanupCompleted };
