import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const root = process.cwd();
const stampPath = path.join(root, "dist", ".build-once-tree");
const command = process.env.BUILD_ONCE_COMMAND || "npm run build";

function git(args, env = process.env, input) {
  return execFileSync("git", args, {
    cwd: root,
    env,
    input,
    stdio: ["pipe", "pipe", "inherit"],
  });
}

// Tree id of the worktree (tracked + untracked, minus ignored) computed in a
// temporary index, so the real index is never touched.
function treeId() {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "build-once-"));
  const tmpIndex = path.join(tmpDir, "index");
  try {
    const env = { ...process.env, GIT_INDEX_FILE: tmpIndex };
    // Keep tracked paths, including staged ignored files, but discard the
    // real index's stat cache so same-size rapid edits are hashed again.
    git(
      ["update-index", "-z", "--index-info"],
      env,
      git(["ls-files", "--stage", "-z"]),
    );
    git(["add", "-A"], env);
    return git(["write-tree"], env).toString("utf8").trim();
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

function writeStamp(id) {
  fs.mkdirSync(path.dirname(stampPath), { recursive: true });
  fs.writeFileSync(stampPath, `${id}\n`);
}

if (process.argv.includes("--stamp")) {
  writeStamp(treeId());
} else {
  const before = treeId();
  let stamped = "";
  try {
    stamped = fs.readFileSync(stampPath, "utf8").trim();
  } catch {
    // no stamp yet
  }
  if (stamped === before) {
    console.log(
      `build-once: tree ${before.slice(0, 12)} already built; skipping ${command}`,
    );
  } else {
    const result = spawnSync(command, {
      cwd: root,
      shell: true,
      stdio: "inherit",
    });
    if (result.status !== 0) process.exit(result.status ?? 1);
    const after = treeId();
    writeStamp(after);
    console.log(`build-once: built tree ${after.slice(0, 12)}`);
  }
}
