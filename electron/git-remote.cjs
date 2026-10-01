const { execFile } = require("node:child_process");
const { promisify } = require("node:util");
const execFileAsync = promisify(execFile);

/** The remote a checkout is known by: the one its current branch tracks, else
 * `origin`, else the first remote it has, whatever it is called. `get-url`
 * resolves `insteadOf` rewrites, so an alias and its expansion match. Empty
 * when the folder has no remote (or is not a repository). */
async function remoteName(cwd, timeout = 5000) {
  const git = async (...args) => {
    try {
      return (
        await execFileAsync("git", ["-C", cwd, ...args], {
          timeout,
          maxBuffer: 100000,
        })
      ).stdout.trim();
    } catch {
      return "";
    }
  };
  const names = (await git("remote")).split("\n").filter(Boolean);
  if (!names.length) return "";
  const branch = await git("symbolic-ref", "-q", "--short", "HEAD");
  const tracked = branch ? await git("config", `branch.${branch}.remote`) : "";
  return [tracked, "origin", names[0]].find((item) => names.includes(item));
}

async function remoteUrl(cwd, timeout = 5000) {
  const name = await remoteName(cwd, timeout);
  if (!name) return "";
  try {
    return (
      await execFileAsync("git", ["-C", cwd, "remote", "get-url", name], {
        timeout,
        maxBuffer: 100000,
      })
    ).stdout.trim();
  } catch {
    return "";
  }
}

module.exports = { remoteUrl, remoteName };
