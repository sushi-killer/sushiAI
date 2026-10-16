/** The primary nav: the app's own sections first, then what a manifest adds in
 * the order the registry gives. The one exception is a negative `order` from a
 * built-in extension: that entry sits right under the first section (Inbox),
 * above the rest. A third-party negative `order` counts as 0. */
export function placePrimaryNav<
  B extends { order: number },
  C extends { order: number; builtin?: boolean },
>(builtin: B[], contributed: C[]): (B | C)[] {
  const above = contributed.filter((item) => item.builtin && item.order < 0);
  const below = contributed.filter((item) => !(item.builtin && item.order < 0));
  return [...builtin.slice(0, 1), ...above, ...builtin.slice(1), ...below];
}
