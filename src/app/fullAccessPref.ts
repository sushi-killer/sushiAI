import { FULL_ACCESS_AGENTS } from "../lib/agent-title.ts";

const KEY = "sushiai.fullAccess.";

/** The last "Full access" choice per agent, off until the owner turns it on. An
 * agent without a full-access mode always reads off. */
export function readFullAccess(agent: string, storage?: Storage): boolean {
  if (!FULL_ACCESS_AGENTS.includes(agent)) return false;
  try {
    return (storage || localStorage).getItem(KEY + agent) === "true";
  } catch {
    // Private windows and cleared site data both throw here; a preference
    // nobody can read is simply off, never on.
    return false;
  }
}

export function writeFullAccess(
  agent: string,
  on: boolean,
  storage?: Storage,
): void {
  if (!FULL_ACCESS_AGENTS.includes(agent)) return;
  try {
    (storage || localStorage).setItem(KEY + agent, String(on));
  } catch {
    // Losing the preference is survivable; refusing the click is not.
  }
}

/** Every supporting agent's remembered choice, for the picker's first render. */
export function readAllFullAccess(storage?: Storage): Record<string, boolean> {
  return Object.fromEntries(
    FULL_ACCESS_AGENTS.map((agent) => [agent, readFullAccess(agent, storage)]),
  );
}
