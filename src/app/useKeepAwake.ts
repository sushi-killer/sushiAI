import { useCallback, useEffect, useState } from "react";

const STORAGE = "sushiai.keepAwake";

export function readKeepAwake(storage?: Storage): boolean {
  try {
    return (storage || localStorage).getItem(STORAGE) === "true";
  } catch {
    // Private windows and cleared site data both throw here; a preference
    // nobody can read is simply off, not a crash on startup.
    return false;
  }
}

export function writeKeepAwake(on: boolean, storage?: Storage): void {
  try {
    (storage || localStorage).setItem(STORAGE, String(on));
  } catch {
    // Losing the preference is survivable; refusing to toggle is not.
  }
}

/**
 * Keeps the Mac awake while sushiAI is running, so a long agent turn is not
 * cut short by idle sleep. The blocker lives in the main process because only
 * it can hold one; this hook owns the preference and re-applies it on mount,
 * which is what makes the setting survive a restart.
 */
export function useKeepAwake(): [boolean, (on: boolean) => void] {
  const [on, setOn] = useState(readKeepAwake);

  useEffect(() => {
    void window.bridge?.keepAwake?.(on);
  }, [on]);

  const set = useCallback((next: boolean) => {
    writeKeepAwake(next);
    setOn(next);
  }, []);

  return [on, set];
}
