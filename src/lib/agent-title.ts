// The one place in the app that reads an agent's id and name for a person.
// Every list, chip, panel title and inbox row reads it from here; a name or an
// id list written out anywhere else is a duplicate that drifts.
//
// The data is `electron/agents.json`, which the main process reads too and a
// Rust test holds equal to the registry (`Harness::label`). `shell` has no
// entry: a plain terminal is titled by its shell (`useWorkspaces`).
//
// ponytail: a synchronous lookup, so the renderer needs no round trip. Step 8
// hands the renderer the daemon's catalog and this becomes a cache of it.
import agents from "../../electron/agents.json" with { type: "json" };

export const AGENT_NAMES: Record<string, string> = Object.fromEntries(
  agents.map((agent) => [agent.id, agent.label]),
);

/** The agents a panel can start, in picker order. */
export const AGENTS = agents.map((agent) => agent.id);

/** The agents the orchestrator, routines and the headless chat can run. Not the
 * json's `native` flag: that one is only "the daemon launches it with hooks". */
export const HARNESSES = ["claude", "codex"] as const;
export type Harness = (typeof HARNESSES)[number];

/** The name to show for an agent id; the id itself for anything unknown. */
export const agentTitle = (name: string) => AGENT_NAMES[name] || name;
