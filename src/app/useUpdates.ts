import { useEffect, useState } from "react";
import { errorText } from "./errors.ts";
import type { UpdateState } from "../types";

/** Live update state. The push subscription wins over the initial fetch, so a
 * state that arrives while the first request is still open is not overwritten
 * by the older snapshot. */
export function useUpdates(notify: (text: string) => void) {
  const [updates, setUpdates] = useState<UpdateState | null>(null);
  useEffect(() => {
    const bridge = window.bridge;
    if (!bridge?.updatesState) return;
    let stopped = false;
    let received = false;
    const unsubscribe = bridge.onUpdates((state) => {
      received = true;
      setUpdates(state);
    });
    bridge
      .updatesState()
      .then((state) => {
        if (!stopped && !received) setUpdates(state);
      })
      .catch((error) => notify(errorText(error)));
    return () => {
      stopped = true;
      unsubscribe();
    };
  }, [notify]);
  return updates;
}
