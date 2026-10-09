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

const HIDDEN = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu;
const hex = (char: string) =>
  `\\u{${(char.codePointAt(0) as number).toString(16)}}`;

/** Text for the card: every control, format (zero-width, bidi), line- and
 * paragraph-separator character is written as \u{hex}, so nothing can hide. */
export function escapeText(text: string): string {
  return text.replace(HIDDEN, hex);
}

/** A word of argv for display. As escapeText, but a newline reads as a visible
 * \n marker followed by a real line break, and a tab as \t, so a script stays
 * readable and every character is still shown. */
export function showWord(word: string): string {
  return word.replace(HIDDEN, (char) =>
    char === "\n" ? "\\n\n" : char === "\t" ? "\\t" : hex(char),
  );
}
