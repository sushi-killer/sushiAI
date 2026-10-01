const {
  parsePreflight,
  PREFLIGHT_SCRIPT,
} = require("./orchestrator-remote.cjs");
const { normalizeRemote, assertRemote } = require("./projects.cjs");

const quote = (value) => `'${String(value).replaceAll("'", "'\\''")}'`;

/** The lock files a project can install from, with the command each one
 * means. The one list: the install default, the hash of what was last
 * installed and the probe all read it. */
const LOCK_FILES = [
  ["package-lock.json", "npm ci"],
  ["pnpm-lock.yaml", "pnpm install --frozen-lockfile"],
  ["yarn.lock", "yarn install --frozen-lockfile"],
  ["bun.lock", "bun install --frozen-lockfile"],
  ["bun.lockb", "bun install --frozen-lockfile"],
  ["Cargo.lock", "cargo build"],
  ["uv.lock", "uv sync --locked"],
  ["poetry.lock", "poetry install"],
  ["Pipfile.lock", "pipenv sync"],
  ["Gemfile.lock", "bundle install"],
  ["composer.lock", "composer install"],
  ["go.sum", "go mod download"],
];

/** The install command a lock file's name implies, or "". */
function installFor(lockFile) {
  return LOCK_FILES.find(([file]) => file === lockFile)?.[1] ?? "";
}

/** Sets `lock_file` and `lock_hash` for the lock file in the current folder. */
const LOCK_HASH = `lock_file=$(find . -maxdepth 1 -type f \\( ${LOCK_FILES.map(([file]) => `-name ${file}`).join(" -o ")} \\) -print -quit)\nif [ -n "$lock_file" ]; then\n  if command -v sha256sum >/dev/null 2>&1; then\n    lock_hash=$(sha256sum "$lock_file" | cut -d ' ' -f 1)\n  else\n    lock_hash=$(shasum -a 256 "$lock_file" | cut -d ' ' -f 1)\n  fi\nelse\n  lock_hash=none\nfi\n`;

/** The folder name a project gets under ~/sushiai on a host. */
function projectSlug(name) {
  return (
    String(name)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") || "project"
  );
}

/** Where the last-installed lock hash is kept: inside the checkout's git
 * folder (found by git, so a worktree or a moved .git works), else a cache
 * of the app's own, never a file in the working tree. */
const HASH_FILE = `hash_file=$(git rev-parse --git-path sushiai-lock-hash 2>/dev/null || true)\n[ -n "$hash_file" ] || hash_file="$HOME/.sushiai/lock-hashes/$(pwd | cksum | cut -d ' ' -f1)"\n`;

/** The enabled stdio MCP servers of a project: name -> command to run. */
function mcpCommands(project) {
  const servers = project?.mcp?.mcpServers ?? {};
  const off = new Set(project?.mcp?.disabledMcpServers ?? []);
  return Object.fromEntries(
    Object.entries(servers)
      .filter(([name, server]) => !off.has(name) && server?.command)
      .map(([name, server]) => [name, String(server.command)]),
  );
}

function hostProbeScript(cwd, projectName = "project", project) {
  const slug = projectSlug(projectName);
  const root = cwd ? quote(cwd) : `"$HOME/sushiai/${slug}"`;
  const commands = Object.values(mcpCommands(project))
    .filter((command) => /^[\w.+-]+$/.test(command))
    .map(
      (command) =>
        `if command -v ${command} >/dev/null 2>&1; then echo "mcpcmd=${command}:1"; else echo "mcpcmd=${command}:0"; fi\n`,
    )
    .join("");
  return `export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"\n${commands}mkdir -p "$HOME/sushiai"\nprintf 'home=%s\\n' "$HOME"\necho "platform=$(uname -sm)"\nprintf 'root=%s\\n' ${root}\ncase ${root} in "$HOME"/sushiai/*) echo "standard=1";; *) echo "standard=0";; esac\necho "remote=$(cd ${root} 2>/dev/null && git remote get-url origin 2>/dev/null || true)"\nif cd ${root} 2>/dev/null; then\n${LOCK_HASH}echo "lockfile=$lock_file"\necho "lockhash=$lock_hash"\n${HASH_FILE}echo "installed=$(cat "$hash_file" 2>/dev/null || true)"\nfi\n${PREFLIGHT_SCRIPT}`;
}

/** Which enabled MCP servers can run on the host: a stdio server whose
 * command the probe did not find there is reported in `missing`. */
function mcpReadiness(project, output) {
  const commands = mcpCommands(project);
  const absent = new Set(
    [...output.matchAll(/^mcpcmd=(.+):0$/gm)].map((match) => match[1]),
  );
  const missing = Object.entries(commands)
    .filter(([, command]) => absent.has(command))
    .map(([name, command]) => ({ name, command }));
  const count = Object.keys(project.mcp?.mcpServers ?? {}).length;
  return { ok: count > 0 && missing.length === 0, count, missing };
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
    platform: values.platform || undefined,
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
      // The next run installs again: the lock file is not the one last
      // installed from. Only known once the probe could read the checkout.
      stale:
        !!project.setup?.install &&
        values.lockhash !== undefined &&
        values.lockhash !== values.installed,
      lockFile: (values.lockfile || "").replace(/^\.\//, "") || undefined,
    },
    clis: parsePreflight(output, now),
    mcp: mcpReadiness(project, String(output)),
    secrets: {
      ok: project.env.some((entry) => entry.secret && entry.hasValue),
      count: project.env.filter((entry) => entry.secret && entry.hasValue)
        .length,
    },
    trusted: !!project.hosts?.[project.host]?.trusted,
  };
}

/** Runs the project's install command in the current folder, unless the lock
 * file is the one it last installed from. */
function installScript(project) {
  return project.setup?.install
    ? `${LOCK_HASH}${HASH_FILE}if [ "$(cat "$hash_file" 2>/dev/null || true)" != "$lock_hash" ]; then\n  ${project.setup.install}\n  # Remembering what was installed is a convenience: failing to write it is\n  # not a failed install.\n  { mkdir -p "$(dirname "$hash_file")" && printf '%s\\n' "$lock_hash" > "$hash_file"; } 2>/dev/null || true\nfi`
    : "";
}

/** What a prepare script reads from stdin; empty when there is nothing. */
function stdinFor(token, setupEnv) {
  const lines = Object.entries(setupEnv)
    .filter(([name]) => /^[A-Za-z_][A-Za-z0-9_]*$/.test(name))
    .map(
      ([name, value]) =>
        `${name}=${Buffer.from(String(value)).toString("base64")}`,
    );
  return token || lines.length ? [token || "", ...lines].join("\n") + "\n" : "";
}

function prepareScript(project, token, setupEnv = {}) {
  assertRemote(project.git.url);
  const slug = projectSlug(project.name);
  const target = `"$HOME/sushiai/${slug}"`;
  // stdin carries, in order: the git token (a line, maybe empty), then one
  // `NAME=base64(value)` line per setup variable. Nothing is on argv or disk.
  const prelude =
    [
      "IFS= read -r git_token || true",
      "setup_names=",
      "while IFS= read -r line; do",
      '  k="${line%%=*}"; v="${line#*=}"',
      "  case \"$k\" in ''|*[!A-Za-z0-9_]*) continue ;; esac",
      '  export "$k=$(printf \'%s\' "$v" | { base64 -d 2>/dev/null || base64 -D; })"',
      '  setup_names="$setup_names $k"',
      "done",
    ].join("\n") + "\n";
  const tokenSetup = token
    ? `export SUSHIAI_GIT_TOKEN="$git_token"\numask 077\nmkdir -p "$HOME/.sushiai"\naskpass=$(mktemp "$HOME/.sushiai/askpass.XXXXXX")\nprintf '%s\\n' '#!/bin/sh' 'case "$1" in *sername*) printf "%s\\n" x-access-token ;; *) printf "%s\\n" "$SUSHIAI_GIT_TOKEN" ;; esac' > "$askpass"\nchmod 700 "$askpass"\nexport GIT_ASKPASS="$askpass" GIT_TERMINAL_PROMPT=0\n`
    : "";
  const tokenCleanup = token
    ? `rm -f "$askpass"\nunset SUSHIAI_GIT_TOKEN git_token GIT_ASKPASS\n`
    : "";
  const install = installScript(project);
  const cloneFresh = `${tokenSetup}if [ -e ${target} ]; then\n  echo "$HOME/sushiai/${slug} exists and is not a checkout" >&2\n  exit 1\nfi\nrm -rf ${target}.prepare\ngit clone --branch ${quote(project.git.defaultBranch || "main")} -- ${quote(project.git.url)} ${target}.prepare\nmv ${target}.prepare ${target}\necho 'SUSHIAI_PULL=cloned'\n${tokenCleanup}`;
  const pullExisting = `${tokenSetup}pull=current\ncd ${target}\nif [ -n "$(git status --porcelain --untracked-files=no 2>/dev/null)" ]; then pull=skipped:local-changes\nelif ! git fetch --quiet origin >/dev/null 2>&1; then pull=skipped:fetch-failed\nelif ! git symbolic-ref -q HEAD >/dev/null 2>&1; then pull=skipped:detached-head\nelif ! git rev-parse -q --verify '@{u}' >/dev/null 2>&1; then pull=skipped:no-upstream\nelse\n  before=$(git rev-parse HEAD)\n  if git pull --ff-only --quiet >/dev/null 2>&1; then\n    [ "$(git rev-parse HEAD)" = "$before" ] || pull=updated\n  else\n    pull=skipped:not-fast-forward\n  fi\nfi\necho "SUSHIAI_PULL=$pull"\ncd "$HOME"\n${tokenCleanup}`;
  const clone = `mkdir -p "$HOME/sushiai"\nif [ ! -e ${target}/.git ]; then\n${cloneFresh}else\n${pullExisting}fi`;
  // Each step prints `SUSHIAI_STEP=<id>:<epoch>` when it finishes, so the
  // caller can tell what ran and how long it took.
  // A marker goes to stdout (read when the run succeeds) and to stderr (all
  // a failed run hands back), so either way the steps can be told apart.
  const mark = (id) =>
    `t=$(date +%s); printf 'SUSHIAI_STEP=${id}:%s\\n' "$t"; printf 'SUSHIAI_STEP=${id}:%s\\n' "$t" >&2`;
  // The step that is starting, on stderr: the last one seen names the step a
  // failure belongs to.
  const stage = (id) => `sushiai_stage=${id}; echo 'SUSHIAI_STAGE=${id}' >&2`;
  // Whatever way the script ends, a failure says which step it was in as its
  // very last line, so a noisy install cannot bury it.
  const guard = `sushiai_stage=clone\ntrap 'rc=$?; rm -f "\${askpass:-}"; unset SUSHIAI_GIT_TOKEN git_token; [ "$rc" -eq 0 ] || echo "SUSHIAI_FAILED=$sushiai_stage" >&2' EXIT\n`;
  const setup = install
    ? `cd ${target}\n${stage("install")}\n${install}\n${mark("install")}`
    : "";
  const check = project.setup?.check
    ? `cd ${target}\n${stage("check")}\n${project.setup.check}\n${mark("check")}`
    : "";
  return {
    script: `export GIT_TERMINAL_PROMPT=0\n${guard}set -e\n${prelude}${mark("start")}\n${stage("clone")}\n${clone}\n${mark("clone")}\n${setup}\n${check}\nunset $setup_names\nprintf 'SUSHIAI_PREPARED=%s\\n' ${target}`,
    input: stdinFor(token, setupEnv),
    path: `~/sushiai/${slug}`,
  };
}

/** The steps a prepare run got through, from its `SUSHIAI_STEP=` markers.
 * `failed` names the step that stopped it; steps after it stay pending. */
function prepareSteps(project, output, failed) {
  const at = {};
  for (const [, id, epoch] of String(output || "").matchAll(
    /^SUSHIAI_STEP=(\w+):(\d+)$/gm,
  ))
    at[id] = Number(epoch);
  const planned = [
    "clone",
    ...(project.setup?.install ? ["install"] : []),
    ...(project.setup?.check ? ["check"] : []),
  ];
  let previous = at.start;
  let stopped = false;
  return planned.map((id) => {
    if (stopped) return { id, state: "pending" };
    if (id === failed) {
      stopped = true;
      return { id, state: "failed" };
    }
    if (at[id] === undefined) return { id, state: "pending" };
    const seconds =
      previous === undefined ? undefined : Math.max(0, at[id] - previous);
    previous = at[id];
    return { id, state: "done", seconds };
  });
}

module.exports = {
  hostProbeScript,
  readiness,
  prepareScript,
  prepareSteps,
  installScript,
  projectSlug,
  LOCK_FILES,
  installFor,
};
