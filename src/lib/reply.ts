// Pure logic of a reply with options: a picked choice and typed text become
// one reply. Shared by the mascot bubble and the Inbox. No React, no bridge.

/** Typed text replaces the pick; a pick alone is sent as is. */
export function composeReply(pick: string, note: string): string {
  return note.trim() || pick;
}

/** The reply state. `pick` is the user's own choice: undefined until they
 * pick, "" once they unpick. `preselected` is the first choice, shown picked
 * up front so one click on send sends it. */
export type ReplyChoice = {
  pick: string | undefined;
  preselected: string;
  note: string;
};

/** The choice a chip shows as picked: none while text is typed (it replaces
 * the pick), else the user's pick, else the preselection. */
export function shownPick(choice: ReplyChoice): string {
  if (choice.note.trim()) return "";
  return choice.pick ?? choice.preselected;
}

/** Clicking the picked choice unpicks it; any other one is picked. */
export function togglePick(choice: ReplyChoice, option: string): string {
  return shownPick(choice) === option ? "" : option;
}

/** What a click on send sends: the preselection counts, a click is
 * deliberate. */
export function clickReply(choice: ReplyChoice): string {
  return composeReply(choice.pick ?? choice.preselected, choice.note);
}

/** How long Enter is ignored after a notice appears, so a held or doubled
 * Enter never replies to the next one. */
export const ENTER_ARM_MS = 500;

/** What Enter sends, "" for nothing: only what the user chose for this
 * notice (an explicit pick or typed text), never the preselection, and
 * nothing within ENTER_ARM_MS of `shownAt`. */
export function enterReply(
  choice: ReplyChoice,
  shownAt: number,
  now: number,
): string {
  if (now - shownAt < ENTER_ARM_MS) return "";
  return composeReply(choice.pick ?? "", choice.note);
}
