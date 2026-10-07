/** Whether a global key belongs to something else: an open dialog, a text
 * field, or - for anything but J/K - a focused button or link, where Enter is
 * that button's own click, not a second answer. */
export function ownsKey(target: EventTarget | null, key: string): boolean {
  if (document.querySelector("[role=dialog], dialog[open]")) return true;
  const el = target as HTMLElement | null;
  if (!el?.closest) return false;
  if (el.isContentEditable || el.closest("input, textarea, select"))
    return true;
  return key !== "j" && key !== "k" && !!el.closest("button, a");
}
