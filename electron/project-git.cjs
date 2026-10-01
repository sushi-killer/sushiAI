const fs = require("node:fs/promises");
const path = require("node:path");
const { execFile } = require("node:child_process");
const { promisify } = require("node:util");
const execFileAsync = promisify(execFile);
const { normalizeRemote, assertRemote } = require("./projects.cjs");

const MESSAGES = {
  cloned: "Cloned",
  updated: "Updated",
  current: "Checkout up to date",
  "skipped:local-changes": "Checkout has local changes, not updated",
  "skipped:different-remote": "Checkout uses a different remote, not updated",
  "skipped:fetch-failed": "Could not fetch, not updated",
  "skipped:detached-head": "Checkout is not on a branch, not updated",
  "skipped:no-upstream": "Branch has no upstream, not updated",
  "skipped:not-fast-forward": "Branch has diverged, not updated",
};

function pullMessage(state) {
  return (
    MESSAGES[state] ||
    (String(state).startsWith("skipped:") ? "Not updated" : "")
  );
}

async function git(cwd, args) {
  return execFileAsync("git", ["-C", cwd, ...args], {
    timeout: 5 * 60 * 1000,
    maxBuffer: 1024 * 1024,
    env: { ...process.env, GIT_TERMINAL_PROMPT: "0" },
  });
}

/** Fast-forward an existing checkout. Never throws for an unsafe or failing
 * pull: the state says why nothing changed, and no work is ever discarded. */
async function pullExisting(cwd, url) {
  const ok = async (...args) => {
    try {
      return (await git(cwd, args)).stdout.trim();
    } catch {
      return null;
    }
  };
  const origin = await ok("remote", "get-url", "origin");
  if (url && normalizeRemote(origin || "") !== normalizeRemote(url))
    return "skipped:different-remote";
  if (await ok("status", "--porcelain", "--untracked-files=no"))
    return "skipped:local-changes";
  if ((await ok("fetch", "--quiet", "origin")) === null)
    return "skipped:fetch-failed";
  if (!(await ok("symbolic-ref", "-q", "HEAD"))) return "skipped:detached-head";
  if ((await ok("rev-parse", "-q", "--verify", "@{u}")) === null)
    return "skipped:no-upstream";
  const before = await ok("rev-parse", "HEAD");
  if ((await ok("pull", "--ff-only", "--quiet")) === null)
    return "skipped:not-fast-forward";
  return (await ok("rev-parse", "HEAD")) === before ? "current" : "updated";
}

async function isCheckout(cwd) {
  try {
    await fs.access(path.join(cwd, ".git"));
    return true;
  } catch {
    return false;
  }
}

async function localCreate({ url, cwd, branch, empty }) {
  if (typeof cwd !== "string" || !path.isAbsolute(cwd))
    throw new Error("Choose an absolute project folder.");
  if (empty) {
    await fs.mkdir(cwd, { recursive: true });
    await git(cwd, ["init"]);
    return { cwd, pull: "current", message: pullMessage("current") };
  }
  if (typeof url !== "string" || !url.trim())
    throw new Error("Enter a git URL.");
  assertRemote(url);
  if (await isCheckout(cwd)) {
    const pull = await pullExisting(cwd, url.trim());
    // Another repository's folder is not this project: stop, do not reuse it.
    if (pull === "skipped:different-remote")
      throw new Error(`${cwd} is a checkout of a different repository.`);
    return { cwd, pull, message: pullMessage(pull) };
  }
  await fs.mkdir(path.dirname(cwd), { recursive: true });
  await execFileAsync("git", ["clone", "--", url.trim(), cwd], {
    timeout: 15 * 60 * 1000,
    env: { ...process.env, GIT_TERMINAL_PROMPT: "0" },
  });
  if (branch && branch !== "main") await git(cwd, ["checkout", branch]);
  return { cwd, pull: "cloned", message: pullMessage("cloned") };
}

module.exports = { localCreate, pullExisting, pullMessage };
