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

function prepareScript(project, token) {
  const slug =
    String(project.name)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") || "project";
  const target = `"$HOME/sushiai/${slug}"`;
  const tokenSetup = token
    ? `IFS= read -r git_token || true\nexport SUSHIAI_GIT_TOKEN="$git_token"\numask 077\nmkdir -p "$HOME/.sushiai"\naskpass=$(mktemp "$HOME/.sushiai/askpass.XXXXXX")\ntrap 'rm -f "$askpass"; unset SUSHIAI_GIT_TOKEN git_token' EXIT\nprintf '%s\\n' '#!/bin/sh' 'case "$1" in *sername*) printf "%s\\n" x-access-token ;; *) printf "%s\\n" "$SUSHIAI_GIT_TOKEN" ;; esac' > "$askpass"\nchmod 700 "$askpass"\nexport GIT_ASKPASS="$askpass" GIT_TERMINAL_PROMPT=0\n`
    : "";
  const tokenCleanup = token
    ? `rm -f "$askpass"\nunset SUSHIAI_GIT_TOKEN git_token GIT_ASKPASS\ntrap - EXIT\n`
    : "";
  const install = project.setup?.install
    ? `lock_file=$(find . -maxdepth 1 -type f \\( -name package-lock.json -o -name pnpm-lock.yaml -o -name yarn.lock -o -name Cargo.lock -o -name poetry.lock -o -name uv.lock -o -name Pipfile.lock -o -name Gemfile.lock -o -name composer.lock -o -name bun.lock -o -name bun.lockb -o -name go.sum \\) -print -quit)\nif [ -n "$lock_file" ]; then\n  if command -v sha256sum >/dev/null 2>&1; then\n    lock_hash=$(sha256sum "$lock_file" | cut -d ' ' -f 1)\n  else\n    lock_hash=$(shasum -a 256 "$lock_file" | cut -d ' ' -f 1)\n  fi\nelse\n  lock_hash=none\nfi\nif [ "$(cat .sushiai-lock-hash 2>/dev/null || true)" != "$lock_hash" ]; then\n  ${project.setup.install}\n  printf '%s\\n' "$lock_hash" > .sushiai-lock-hash\nfi`
    : "";
  const clone = `mkdir -p "$HOME/sushiai"\nif [ ! -d ${target}/.git ]; then\n${tokenSetup}rm -rf ${target}.prepare\nif ! git clone --branch ${quote(project.git.defaultBranch || "main")} -- ${quote(project.git.url)} ${target}.prepare; then\n  echo 'SUSHIAI_PREPARE_STAGE=clone' >&2\n  exit 1\nfi\nmv ${target}.prepare ${target}\n${tokenCleanup}fi`;
  const setup = install ? `cd ${target}\n${install}` : "";
  const check = project.setup?.check
    ? `cd ${target}\n${project.setup.check}`
    : "";
  return {
    script: `export GIT_TERMINAL_PROMPT=0\nset -e\n${clone}\n${setup}\n${check}\nprintf 'SUSHIAI_PREPARED=%s\\n' ${target}`,
    input: token ? `${token}\n` : "",
    path: `~/sushiai/${slug}`,
  };
}

module.exports = { hostProbeScript, readiness, prepareScript };
