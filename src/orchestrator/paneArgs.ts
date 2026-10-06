import type { OrchestratorView } from "../types.ts";

/** The pane's saved state is a string map; the view is kept as JSON. */
export function parseView(
  text: string | undefined,
): OrchestratorView | undefined {
  if (!text) return undefined;
  try {
    const value = JSON.parse(text);
    return value && typeof value.kind === "string" ? value : undefined;
  } catch {
    return undefined;
  }
}

/** Returns `args` with `patch` applied; an undefined value removes the key. */
export function patchArgs(
  args: Record<string, string>,
  patch: Record<string, string | undefined>,
): Record<string, string> {
  const next = { ...args };
  for (const [key, value] of Object.entries(patch)) {
    if (value === undefined) delete next[key];
    else next[key] = value;
  }
  return next;
}
