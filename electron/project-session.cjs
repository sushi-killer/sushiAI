const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { quote } = require("./connections.cjs");

/** The project a folder on a host belongs to: the one the folder was attached
 * to, else the one of its git remote. Null when it has none. */
async function projectForFolder({ projects, connections }, endpoint, cwd) {
  if (!projects || typeof cwd !== "string" || !cwd) return null;
  const host =
    typeof endpoint === "string" && endpoint.startsWith("ssh:")
      ? endpoint
      : "local";
  const byFolder = await projects.resolveFolder({ endpoint: host, cwd });
  if (byFolder) return byFolder;
  const info = await connections
    ?.inspect(host === "local" ? undefined : host, {
      operation: "git_remote",
      root: cwd,
    })
    .catch(() => null);
  return info?.remote ? projects.resolve(info.remote) : null;
}

/** Exported lines for a shell to source: one `export NAME='value'` each. */
function envPayload(env) {
  return Object.entries(env || {})
    .map(([name, value]) => `export ${name}=${quote(value)}`)
    .join("\n");
}

/** What to type into a Herdr pane so the project's values are in its shell:
 * the values go into a one-shot 0600 file (on the host, over ssh stdin; here
 * for This Mac), and the text typed only sources and removes that file, so
 * no value is ever in the text, the pane's history or argv. Empty when the
 * folder has no project, no values, or the host is switched off. */
async function sessionEnvPrefix(
  { projects, connections, upload, remove },
  { endpoint, cwd },
) {
  const project = await projectForFolder(
    { projects, connections },
    endpoint,
    cwd,
  );
  if (!project) return "";
  const remote = typeof endpoint === "string" && endpoint.startsWith("ssh:");
  const host = remote ? endpoint : "local";
  if (!(await projects.sendsValues(project.id, host))) return "";
  const payload = envPayload(
    await projects.environmentFor(project.id, "agent", host),
  );
  if (!payload) return "";
  let file;
  if (remote) {
    file = await upload(endpoint, payload);
    // A file nothing sourced does not stay: gone after two minutes.
    const timer = setTimeout(() => void remove(endpoint, file), 120000);
    timer.unref?.();
  } else {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "sushiai-env-"));
    file = path.join(dir, "env");
    await fs.writeFile(file, payload, { mode: 0o600 });
    const timer = setTimeout(
      () => void fs.rm(dir, { recursive: true, force: true }),
      120000,
    );
    timer.unref?.();
    return `. ${quote(file)}; rm -rf ${quote(dir)}; `;
  }
  return `. ${quote(file)}; rm -f ${quote(file)}; `;
}

module.exports = { projectForFolder, sessionEnvPrefix, envPayload };
