import type { Panel } from "../types";

/** A write to a surface's args: each key is set, and an undefined value
 * deletes the key. */
export type ArgsPatch = Record<string, string | undefined>;

/** `current` with `patch` applied. */
export function mergeArgs(
  current: Record<string, string> | undefined,
  patch: ArgsPatch,
): Record<string, string> {
  const next = { ...current };
  for (const [key, value] of Object.entries(patch)) {
    if (value === undefined) delete next[key];
    else next[key] = value;
  }
  return next;
}

/** A panel update that merges `patch` into the extension pane's args as they
 * are when the update runs, so two writes in one tick both land. */
export const extensionArgsUpdate =
  (patch: ArgsPatch) =>
  (panel: Panel): Partial<Panel> =>
    panel.kind === "extension"
      ? {
          extension: {
            ...panel.extension,
            args: mergeArgs(panel.extension.args, patch),
          },
        }
      : {};
