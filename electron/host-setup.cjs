const { execFile } = require("node:child_process");

/** Brings a machine to what sessions need - Herdr running, Claude Code and
 * Codex on the PATH - installing only what is missing, from each tool's own
 * installer or release. Prints one `SUSHIAI_SETUP <tool> <state>` line per
 * tool; installers' chatter goes to stderr. Safe to run again at any time. */
const SETUP_SCRIPT = `set -u
export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"
have() { command -v "$1" >/dev/null 2>&1; }
say() { printf 'SUSHIAI_SETUP %s %s\\n' "$1" "$2"; }
have curl || { say curl missing; exit 0; }
if have herdr; then say herdr present
elif curl -fsSL https://herdr.dev/install.sh | sh >&2 && have herdr; then say herdr installed
else say herdr failed; fi
if have claude; then say claude present
elif curl -fsSL https://claude.ai/install.sh | bash >&2 && have claude; then say claude installed
else say claude failed; fi
if have codex; then say codex present
else
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) t=x86_64-unknown-linux-musl ;;
    Linux-aarch64) t=aarch64-unknown-linux-musl ;;
    Darwin-arm64) t=aarch64-apple-darwin ;;
    Darwin-x86_64) t=x86_64-apple-darwin ;;
    *) t= ;;
  esac
  d=$(mktemp -d)
  if [ -n "$t" ] && curl -fsSL "https://github.com/openai/codex/releases/latest/download/codex-$t.tar.gz" | tar -xz -C "$d" >&2 \\
    && mkdir -p "$HOME/.local/bin" && mv "$d/codex-$t" "$HOME/.local/bin/codex" && chmod +x "$HOME/.local/bin/codex"; then
    say codex installed
  else say codex failed; fi
  rm -rf "$d"
fi
if have herdr; then
  if herdr status server 2>/dev/null | grep -q '^status: running'; then say server running
  else
    nohup herdr server >/dev/null 2>&1 </dev/null &
    for i in 1 2 3 4 5 6 7 8 9 10; do
      herdr status server 2>/dev/null | grep -q '^status: running' && break
      sleep 1
    done
    if herdr status server 2>/dev/null | grep -q '^status: running'; then say server started
    else say server failed; fi
  fi
fi
`;

/** `{ herdr: "installed", claude: "present", ... }` from the script's output. */
function parseSetup(output) {
  const states = {};
  for (const [, tool, state] of String(output).matchAll(
    /^SUSHIAI_SETUP (\S+) (\S+)$/gm,
  ))
    states[tool] = state;
  return states;
}

/** One line for the owner: what was installed or started, and what failed. */
function setupSummary(states) {
  const done = Object.entries(states)
    .filter(([, state]) => ["installed", "started"].includes(state))
    .map(([tool, state]) => `${tool} ${state}`);
  const failed = Object.entries(states)
    .filter(([, state]) => ["failed", "missing"].includes(state))
    .map(([tool, state]) => `${tool} ${state}`);
  return [...done, ...failed].join(" · ");
}

/** Runs the setup on an SSH host (over its connection) or on the local machine. */
async function setupHost(connections, endpoint) {
  const timeout = 10 * 60 * 1000;
  const output =
    typeof endpoint === "string" && endpoint.startsWith("ssh:")
      ? await connections.exec(endpoint, "sh -s", {
          input: SETUP_SCRIPT,
          timeout,
        })
      : await new Promise((resolve, reject) => {
          const child = execFile(
            "/bin/sh",
            ["-s"],
            { timeout, maxBuffer: 4 * 1024 * 1024 },
            (error, stdout) => (error ? reject(error) : resolve(stdout)),
          );
          child.stdin.end(SETUP_SCRIPT);
        });
  return parseSetup(output);
}

module.exports = { SETUP_SCRIPT, parseSetup, setupSummary, setupHost };
