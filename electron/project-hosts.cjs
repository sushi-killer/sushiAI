const { parsePreflight, PREFLIGHT_SCRIPT } = require("./host-setup.cjs");
const { normalizeRemote, assertRemote } = require("./projects.cjs");
const { projectSlug, slugOf } = require("./project-slug.cjs");
const { prepareGitUrl, gitSshEnv } = require("./project-git-ssh.cjs");

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
/** The remote a checkout is known by (its branch's upstream remote, else
 * origin, else the first one), as shell that runs in the checkout. */
const REMOTE_NAME_SH = `b=$(git symbolic-ref -q --short HEAD 2>/dev/null); u=$(git config "branch.$b.remote" 2>/dev/null); if [ -z "$u" ] || [ "$u" = . ]; then u=origin; fi; git remote | grep -qx "$u" || u=$(git remote | head -n 1); printf '%s' "$u"`;
const REMOTE_URL_SH = `n=$(${REMOTE_NAME_SH}); [ -n "$n" ] && git remote get-url "$n" 2>/dev/null`;

// The first lock file in the list's own order decides the install, so that is
// the one hashed (never whichever one a directory scan meets first).
const LOCK_HASH = `lock_file=\nfor f in ${LOCK_FILES.map(([file]) => file).join(" ")}; do\n  if [ -f "./$f" ]; then lock_file="./$f"; break; fi\ndone\nif [ -n "$lock_file" ]; then\n  if command -v sha256sum >/dev/null 2>&1; then\n    lock_hash=$(sha256sum "$lock_file" | cut -d ' ' -f 1)\n  else\n    lock_hash=$(shasum -a 256 "$lock_file" | cut -d ' ' -f 1)\n  fi\nelse\n  lock_hash=none\nfi\n`;

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

/** Where a checkout of some repository may already be: the home folder, the
 * usual project folders, and one level below each. Prints one
 * `FOUND=<path><TAB><remote>` line per git checkout; nothing is followed into
 * node_modules, .git or a symlink, and nothing is searched deeper. */
const FIND_CHECKOUTS_SH = `for root in "$HOME" "$HOME/code" "$HOME/projects" "$HOME/src" "$HOME/dev" "$HOME/work" "$HOME/Desktop" "$HOME/Documents"; do
  [ -d "$root" ] || continue
  for d in "$root" "$root"/*/; do
    d=\${d%/}
    case "$d" in */node_modules|*/.git) continue ;; esac
    [ -L "$d" ] && continue
    [ -d "$d" ] && [ -e "$d/.git" ] || continue
    u=$(cd "$d" 2>/dev/null && { ${REMOTE_URL_SH}; }) || continue
    printf 'FOUND=%s\t%s\n' "$d" "$u"
  done
done
exit 0`;

function hostProbeScript(cwd, projectName = "project", project) {
  const slug = project ? slugOf(project) : projectSlug(projectName);
  const root = cwd ? quote(cwd) : `"$HOME/sushiai/${slug}"`;
  const commands = Object.values(mcpCommands(project))
    .filter((command) => /^[\w.+-]+$/.test(command))
    .map(
      (command) =>
        `if command -v ${command} >/dev/null 2>&1; then echo "mcpcmd=${command}:1"; else echo "mcpcmd=${command}:0"; fi\n`,
    )
    .join("");
  return `export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"\n${commands}mkdir -p "$HOME/sushiai"\nprintf 'home=%s\\n' "$HOME"\necho "platform=$(uname -sm)"\nprintf 'root=%s\\n' ${root}\ncase ${root} in "$HOME"/sushiai/*) echo "standard=1";; *) echo "standard=0";; esac\necho "remote=$(cd ${root} 2>/dev/null && { ${REMOTE_URL_SH}; } || true)"\nif cd ${root} 2>/dev/null; then\n${LOCK_HASH}echo "lockfile=$lock_file"\necho "lockhash=$lock_hash"\n${HASH_FILE}echo "installed=$(cat "$hash_file" 2>/dev/null || true)"\nfi\n${PREFLIGHT_SCRIPT}`;
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
    withheld: !!project.hosts?.[project.host]?.withheld,
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

function prepareScript(
  project,
  token,
  setupEnv = {},
  where = "",
  options = {},
) {
  assertRemote(project.git.url);
  const gitUrl = prepareGitUrl(project, options.gitUrl);
  assertRemote(gitUrl);
  const slug = slugOf(project);
  // An existing checkout of this repository elsewhere on the host is used
  // where it is; only a missing one is cloned, into the standard folder.
  const target = where ? quote(where) : `"$HOME/sushiai/${slug}"`;
  // stdin carries, in order: the git token (a line, maybe empty), then one
  // `NAME=base64(value)` line per setup variable. Nothing is on argv or disk:
  // the lines are split with parameter expansion, because a heredoc or
  // here-string is backed by a temp file in zsh, bash < 5.1 and /bin/sh.
  // The setup variables are held, not exported, until the clone is done: a
  // failed clone never ran a command that could see them.
  const prelude =
    "IFS= read -r git_token || true\nsetup_input=$(cat)\nsetup_names=\n";
  const exportSetup = [
    "nl='",
    "'",
    'rest="$setup_input$nl"',
    'while [ -n "$rest" ]; do',
    '  line=${rest%%"$nl"*}; rest=${rest#*"$nl"}',
    '  k="${line%%=*}"; v="${line#*=}"',
    "  case \"$k\" in ''|*[!A-Za-z0-9_]*) continue ;; esac",
    '  export "$k=$(printf \'%s\' "$v" | { base64 -d 2>/dev/null || base64 -D; })"',
    '  setup_names="$setup_names$k$nl"',
    "done",
    "unset setup_input rest line k v",
  ].join("\n");
  const tokenSetup = token
    ? `export SUSHIAI_GIT_TOKEN="$git_token"\numask 077\nmkdir -p "$HOME/.sushiai"\naskpass=$(mktemp "$HOME/.sushiai/askpass.XXXXXX")\nprintf '%s\\n' '#!/bin/sh' 'case "$1" in *sername*) printf "%s\\n" x-access-token ;; *) printf "%s\\n" "$SUSHIAI_GIT_TOKEN" ;; esac' > "$askpass"\nchmod 700 "$askpass"\nexport GIT_ASKPASS="$askpass" GIT_TERMINAL_PROMPT=0\n`
    : "";
  const tokenCleanup = token
    ? `rm -f "$askpass"\nunset SUSHIAI_GIT_TOKEN git_token GIT_ASKPASS\n`
    : "";
  const install = installScript(project);
  const cloneFresh = `${tokenSetup}${gitSshEnv(gitUrl)}if [ -e ${target} ]; then\n  echo "The target folder exists and is not a checkout" >&2\n  exit 1\nfi\nclone_directory=$(mktemp -d ${target}.prepare.XXXXXX)\ngit clone ${project.git.defaultBranch ? `--branch ${quote(project.git.defaultBranch)} ` : ""}-- ${quote(gitUrl)} "$clone_directory"\nif [ -e ${target} ]; then echo 'The target folder already exists; nothing was replaced.' >&2; exit 1; fi\nmkdir -m 700 ${target}\nreserved_target=${target}\n(cd "$clone_directory" && find . ! -name . -prune -exec mv {} ${target}/ \\;)\nrmdir "$clone_directory"\nreserved_target=\nclone_directory=\necho 'SUSHIAI_PULL=cloned'\nunset GIT_SSH_COMMAND GIT_SSH_VARIANT\n${tokenCleanup}`;
  const pullExisting = `${tokenSetup}${gitSshEnv(gitUrl)}git_url=${quote(gitUrl)}\npull=current\ncd ${target}\nremote_name="$(${REMOTE_NAME_SH})" || true\nif [ -n "$(git status --porcelain --untracked-files=no 2>/dev/null)" ]; then pull=skipped:local-changes\nelif ! git ${options.gitUrl ? '-c "remote.$remote_name.url=$git_url" ' : ""}fetch --quiet "$(${REMOTE_NAME_SH})" >/dev/null 2>&1; then pull=skipped:fetch-failed\nelif ! git symbolic-ref -q HEAD >/dev/null 2>&1; then pull=skipped:detached-head\nelif ! git rev-parse -q --verify '@{u}' >/dev/null 2>&1; then pull=skipped:no-upstream\nelse\n  before=$(git rev-parse HEAD)\n  if git ${options.gitUrl ? '-c "remote.$remote_name.url=$git_url" ' : ""}pull --ff-only --quiet >/dev/null 2>&1; then\n    [ "$(git rev-parse HEAD)" = "$before" ] || pull=updated\n  else\n    pull=skipped:not-fast-forward\n  fi\nfi\necho "SUSHIAI_PULL=$pull"\ncd "$HOME"\nunset GIT_SSH_COMMAND GIT_SSH_VARIANT\n${tokenCleanup}`;
  // A start that only needs the install leaves the checkout's history alone.
  const keep = `echo 'SUSHIAI_PULL=skipped:not-asked'\n`;
  const clone = `mkdir -p "$HOME/sushiai"\nif [ ! -e ${target}/.git ]; then\n${cloneFresh}else\n${options.pull === false ? keep : pullExisting}fi`;
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
  const guard = `sushiai_stage=clone\ntrap 'rc=$?; rm -f "\${askpass:-}"; [ -z "\${clone_directory:-}" ] || rm -rf "$clone_directory"; [ -z "\${reserved_target:-}" ] || rm -rf "$reserved_target"; unset SUSHIAI_GIT_TOKEN git_token; [ "$rc" -eq 0 ] || echo "SUSHIAI_FAILED=$sushiai_stage" >&2' EXIT\n`;
  const setup = install
    ? `cd ${target}\n${stage("install")}\n${install}\n${mark("install")}`
    : "";
  const check = project.setup?.check
    ? `cd ${target}\n${stage("check")}\n${project.setup.check}\n${mark("check")}`
    : "";
  return {
    script: `export GIT_TERMINAL_PROMPT=0\n${guard}set -e\n${prelude}${mark("start")}\n${stage("clone")}\n${clone}\n${mark("clone")}\n${exportSetup}\n${setup}\n${check}\nrest="$setup_names"\nwhile [ -n "$rest" ]; do k=\${rest%%"$nl"*}; rest=\${rest#*"$nl"}; unset "$k"; done\nprintf 'SUSHIAI_PREPARED=%s\\n' ${target}`,
    input: stdinFor(token, setupEnv),
    path: where || `~/sushiai/${slug}`,
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
  REMOTE_URL_SH,
  hostProbeScript,
  FIND_CHECKOUTS_SH,
  readiness,
  prepareScript,
  prepareSteps,
  installScript,
  projectSlug,
  LOCK_FILES,
  installFor,
};
