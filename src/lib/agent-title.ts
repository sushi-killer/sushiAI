// The one place in the app that writes an agent's name for a person. Every
// list, chip, panel title and inbox row reads it from here; a name written out
// anywhere else is a duplicate that drifts.
//
// The values are the CLI's own product names, and they are the same strings the
// Rust registry prints (`sushiai agents catalog`, `Harness::label`). `shell` has
// no name of its own here: a plain terminal is titled by its shell
// (`useWorkspaces`), never by this map.
//
// ponytail: a synchronous lookup, so the renderer needs no round trip. Step 8
// hands the renderer the daemon's catalog and this becomes a cache of it.
export const AGENT_NAMES: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  gemini: "Gemini CLI",
  "cursor-agent": "Cursor Agent",
};

/** The agents a panel can start, in picker order. */
export const AGENTS = Object.keys(AGENT_NAMES);

/** The name to show for an agent id; the id itself for anything unknown. */
export const agentTitle = (name: string) => AGENT_NAMES[name] || name;
