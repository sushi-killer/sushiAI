import type { Project } from "./types.ts";

type Imported = { name: string; secret: boolean; choice: string };

/** What importing a .env file changes about a project's variables, from the
 * stored state. A secret never becomes plain through an import (that would
 * drop its value and its host overrides); an override only adds the host. */
export function importChanges(
  stored: Project["env"],
  imported: Imported[],
  overrideHost: string,
): Project["env"] {
  const set: Project["env"] = [];
  for (const entry of imported) {
    const existing = stored.find((item) => item.name === entry.name);
    if (!existing)
      set.push({
        name: entry.name,
        secret: entry.secret,
        availableTo: ["setup", "agent"],
      });
    else if (entry.choice === "override")
      set.push({
        ...existing,
        hosts: [...new Set([...(existing.hosts || []), overrideHost])],
        secret: existing.secret || entry.secret,
      });
    else set.push({ ...existing, secret: existing.secret || entry.secret });
  }
  return set;
}
