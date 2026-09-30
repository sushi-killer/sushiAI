const {
  parsePreflight,
  PREFLIGHT_SCRIPT,
} = require("./orchestrator-remote.cjs");
const { normalizeRemote } = require("./projects.cjs");

const quote = (value) => `'${String(value).replaceAll("'", "'\\''")}'`;

function hostProbeScript(cwd, projectName = "project") {
  const slug =
    String(projectName)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") || "project";
  const root = cwd ? quote(cwd) : `"$HOME/sushiai/${slug}"`;
  return `mkdir -p "$HOME/sushiai"\nprintf 'home=%s\\n' "$HOME"\nprintf 'root=%s\\n' ${root}\ncase ${root} in "$HOME"/sushiai/*) echo "standard=1";; *) echo "standard=0";; esac\necho "remote=$(cd ${root} 2>/dev/null && git remote get-url origin 2>/dev/null || true)"\n${PREFLIGHT_SCRIPT}`;
}

function readiness({ output, project, cwd, now = Date.now() }) {
  const values = Object.fromEntries(
    String(output)
      .split("\n")
      .map((line) => {
        const at = line.indexOf("=");
        return at < 0 ? ["", ""] : [line.slice(0, at), line.slice(at + 1)];
      }),
  );
  const standard = values.standard === "1";
  const remote = values.remote || "";
  return {
    checkout: {
      ok:
        !!remote &&
        normalizeRemote(remote) === normalizeRemote(project.git?.url),
      path: cwd || values.root || "",
      nonStandard: !!(values.root || cwd) && !standard,
    },
    setup: {
      ok: !!(project.setup?.install || project.setup?.check),
      configured: !!(project.setup?.install || project.setup?.check),
    },
    clis: parsePreflight(output, now),
    mcp: {
      ok: Object.keys(project.mcp || {}).length > 0,
      count: Object.keys(project.mcp || {}).length,
    },
    secrets: {
      ok: project.env.some((entry) => entry.secret && entry.hasValue),
      count: project.env.filter((entry) => entry.secret && entry.hasValue)
        .length,
    },
    trusted: !!project.hosts?.[project.host]?.trusted,
  };
}

module.exports = { hostProbeScript, readiness };
