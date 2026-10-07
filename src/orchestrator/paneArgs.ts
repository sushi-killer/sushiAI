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
