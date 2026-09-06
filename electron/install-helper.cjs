// Runs outside the application bundle so replacement can finish after sushiAI quits.
const fs = require("node:fs/promises");
const path = require("node:path");
const os = require("node:os");
const { createReadStream, constants } = require("node:fs");
const { createHash } = require("node:crypto");
const { execFile } = require("node:child_process");
const { promisify } = require("node:util");
const execute = promisify(execFile);
const BUNDLE_ID = "local.sushiai.workspace";
async function command(file, args) {
  const env = { ...process.env };
  delete env.ELECTRON_RUN_AS_NODE;
  const { stdout } = await execute(file, args, {
    timeout: 180000,
    maxBuffer: 1024 * 1024,
    env,
  });
  return stdout.trim();
}
async function checksum(file) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest("hex");
}
async function verifyBundle(app, expectedVersion, run) {
  const info = path.join(app, "Contents", "Info.plist");
  const id = await run("/usr/libexec/PlistBuddy", [
    "-c",
    "Print :CFBundleIdentifier",
    info,
  ]);
  if (id !== BUNDLE_ID)
    throw Error("The package is not a sushiAI application.");
  if (expectedVersion) {
    const version = await run("/usr/libexec/PlistBuddy", [
      "-c",
      "Print :CFBundleShortVersionString",
      info,
    ]);
    if (version !== expectedVersion)
      throw Error("The application version does not match the release.");
  }
  await run("/usr/bin/codesign", ["--verify", "--deep", "--strict", app]);
}
async function prepare(plan, { run = command } = {}) {
  if (
    !path.isAbsolute(plan.target) ||
    !plan.target.endsWith(".app") ||
    !path.isAbsolute(plan.dmg) ||
    !/^[a-f0-9]{64}$/.test(plan.sha256) ||
    !Number.isSafeInteger(plan.pid) ||
    plan.pid < 1
  )
    throw Error("Invalid installation plan.");
  const target = await fs.realpath(plan.target);
  if (
    target !== plan.target ||
    target.startsWith("/Volumes/") ||
    target.includes("/AppTranslocation/")
  )
    throw Error("Move sushiAI to Applications before installing updates.");
  try {
    await fs.access(path.dirname(target), constants.W_OK);
  } catch {
    throw Error(
      "This Applications folder is not writable. Move sushiAI to your personal Applications folder and try again.",
    );
  }
  if ((await checksum(plan.dmg)) !== plan.sha256)
    throw Error("The downloaded update failed SHA-256 verification.");
  await verifyBundle(target, null, run);
  const directory = await fs.mkdtemp(
    path.join(path.dirname(target), ".sushiAI-update-"),
  );
  const mount = await fs.mkdtemp(
    path.join(os.tmpdir(), "sushiai-update-mount-"),
  );
  const staged = path.join(directory, "next.app");
  let attached = false;
  try {
    attached = true;
    await run("/usr/bin/hdiutil", [
      "attach",
      "-readonly",
      "-nobrowse",
      "-mountpoint",
      mount,
      plan.dmg,
    ]);
    const source = path.join(mount, "sushiAI.app");
    if ((await fs.lstat(source)).isSymbolicLink())
      throw Error("Invalid application in update package.");
    await verifyBundle(source, plan.version, run);
    await run("/usr/bin/ditto", [source, staged]);
    await verifyBundle(staged, plan.version, run);
    await run("/usr/bin/hdiutil", ["detach", mount]);
    attached = false;
    return { directory, staged, backup: path.join(directory, "previous.app") };
  } catch (error) {
    await fs.rm(directory, { recursive: true, force: true });
    throw error;
  } finally {
    if (attached)
      await run("/usr/bin/hdiutil", ["detach", mount]).catch(() => {});
    // rmdir cannot walk an image if detaching failed.
    await fs.rmdir(mount).catch(() => {});
  }
}
function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return e.code !== "ESRCH";
  }
}
async function replace(
  plan,
  prepared,
  {
    run = command,
    isAlive = alive,
    sleep = (ms) => new Promise((r) => setTimeout(r, ms)),
    rename = fs.rename,
  } = {},
) {
  let moved = false;
  let installed = false;
  try {
    for (let i = 0; isAlive(plan.pid); i++) {
      if (i >= 60)
        throw Error("sushiAI did not exit. Close it and try installing again.");
      await sleep(1000);
    }
    await rename(plan.target, prepared.backup);
    moved = true;
    await rename(prepared.staged, plan.target);
    installed = true;
    await fs.writeFile(
      plan.receipt,
      JSON.stringify({
        target: plan.target,
        directory: prepared.directory,
        version: plan.version,
      }),
      { mode: 0o600 },
    );
    await run("/usr/bin/open", ["-n", plan.target]);
    // Keep the backup until the new application successfully loads its window.
  } catch (error) {
    if (moved) {
      try {
        if (installed) await fs.rename(plan.target, prepared.staged);
        await fs.rename(prepared.backup, plan.target);
        moved = false;
        await fs.rm(plan.receipt, { force: true });
      } catch (rollback) {
        throw Error(
          `${error.message} Recovery copy: ${prepared.backup}. ${rollback.message}`,
        );
      }
      await run("/usr/bin/open", ["-n", plan.target]).catch(() => {});
    }
    throw error;
  } finally {
    if (!moved)
      await fs.rm(prepared.directory, { recursive: true, force: true });
  }
}
async function cleanupCompleted(receipt, target, version) {
  try {
    const result = JSON.parse(await fs.readFile(receipt, "utf8"));
    if (
      result.target !== target ||
      result.version !== version ||
      path.dirname(result.directory) !== path.dirname(target) ||
      !/^\.sushiAI-update-[a-zA-Z0-9]+$/.test(path.basename(result.directory))
    )
      return;
    await fs.rm(result.directory, { recursive: true, force: true });
    await fs.rm(receipt, { force: true });
  } catch {
    /* Keep the backup if cleanup is unavailable. */
  }
}
if (require.main === module) {
  process.stdout.on("error", () => {});
  process.stderr.on("error", () => {});
  (async () => {
    const plan = JSON.parse(await fs.readFile(process.argv[2], "utf8"));
    try {
      const prepared = await prepare(plan);
      process.stdout.write("READY\n");
      await replace(plan, prepared);
    } catch (error) {
      await fs
        .writeFile(plan.errorFile, error.message, { mode: 0o600 })
        .catch(() => {});
      process.stderr.write(error.message + "\n");
      process.exitCode = 1;
    } finally {
      const directory = path.dirname(process.argv[2]);
      if (/^install-[a-zA-Z0-9]+$/.test(path.basename(directory)))
        await fs
          .rm(directory, { recursive: true, force: true })
          .catch(() => {});
    }
  })().catch((e) => {
    process.stderr.write(e.message + "\n");
    process.exitCode = 1;
  });
}
module.exports = { prepare, replace, cleanupCompleted };
