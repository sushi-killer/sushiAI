import { useState, type Dispatch, type SetStateAction } from "react";

// The page unmounts when the owner opens another tab. Its working state lives
// here, for the life of the window, so coming back finds the page as it was.
const kept = new Map<string, unknown>();

/** `useState` whose value survives the page being unmounted. */
export function usePageState<T>(
  key: string,
  initial: T,
): [T, Dispatch<SetStateAction<T>>] {
  const [value, set] = useState<T>(() =>
    kept.has(key) ? (kept.get(key) as T) : initial,
  );
  const setKept: Dispatch<SetStateAction<T>> = (next) =>
    set((current) => {
      const resolved =
        typeof next === "function" ? (next as (value: T) => T)(current) : next;
      kept.set(key, resolved);
      return resolved;
    });
  return [value, setKept];
}

/** Drops kept values whose key starts with `prefix`. */
export function forgetPageState(prefix: string) {
  for (const key of [...kept.keys()])
    if (key.startsWith(prefix)) kept.delete(key);
}
