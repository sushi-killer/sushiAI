/** How many lines the composer grows to before it scrolls. */
export const COMPOSER_MAX_ROWS = 6;

/** The pixel height of a composer field: its content (`scrollHeight`, which
 * includes the vertical padding), never less than one line and never more
 * than `maxRows` lines. `overflow` says whether the field must scroll. */
export function composerHeight(
  scrollHeight: number,
  lineHeight: number,
  padding: number,
  maxRows = COMPOSER_MAX_ROWS,
): { height: number; overflow: boolean } {
  const min = lineHeight + padding;
  const max = lineHeight * maxRows + padding;
  const height = Math.min(Math.max(scrollHeight, min), max);
  return { height, overflow: scrollHeight > max };
}
