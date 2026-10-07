/** "1 file", "2 files": a count and its noun, which takes an "s" unless the
 * count is one. */
export function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

/** "2m", "14m", "1h", "3d": how long ago. Under a minute reads "0m"; callers
 * that want a word for it check first. */
export function elapsedLabel(ms: number): string {
  const minutes = Math.max(0, Math.floor(ms / 60_000));
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours}h` : `${Math.floor(hours / 24)}d`;
}
