/** How long the Allow button stays disabled after a card appears, so a click
 * meant for something underneath cannot allow a command. */
export const ALLOW_DELAY_MS = 1000;

/** What a key does on the owner card. Enter and Escape deny; Allow needs a
 * click, or Tab to the button and Space. Enter on the Allow button does
 * nothing, so a stray Enter can never allow. */
export function execKeyAction(
  key: string,
  onAllow: boolean,
): "deny" | "ignore" | "default" {
  if (key === "Escape") return "deny";
  if (key === "Enter") return onAllow ? "ignore" : "deny";
  return "default";
}

/** A word of argv for display: exactly as sent, except that control and
 * bidirectional-override characters (other than newline and tab) are written
 * as \uXXXX, so nothing can hide in the text. */
export function showWord(word: string): string {
  return word.replace(
    // eslint-disable-next-line no-control-regex
    /[\u0000-\u0008\u000b-\u001f\u007f-\u009f​-‏‪-‮⁦-⁩﻿]/g,
    (char) => `\\u${char.charCodeAt(0).toString(16).padStart(4, "0")}`,
  );
}
