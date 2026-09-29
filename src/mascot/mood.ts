import type { MascotNotice } from "./types";

export type Mood =
  "idle" | "working" | "needs-you" | "listening" | "done" | "failed";

/** The character's mood for the notice on show. `typing` is true while the
 * bubble's reply field is focused or holds text. */
export function mascotMood(
  notice: Pick<MascotNotice, "kind"> & { answered?: boolean },
  typing: boolean,
): Mood {
  if (notice.kind === "core-update") return "idle";
  if (notice.answered) return "working";
  switch (notice.kind) {
    case "input":
      return typing ? "listening" : "needs-you";
    case "done":
      return "done";
    case "failed":
    case "stopped":
      return "failed";
    default:
      return "idle";
  }
}

/** Whether the owner is typing, tracked per notice id so a state left behind
 * by another notice (or an unmounted field) never leaks into the next one. */
export type Typing = { id: string; on: boolean };

export const isTyping = (
  state: Typing,
  notice: { id: string; answered?: boolean },
): boolean => state.id === notice.id && state.on && !notice.answered;
