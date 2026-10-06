import { useEffect, useState } from "react";
import {
  applyAskEvent,
  applyAskList,
  noAsks,
  type AsksByHost,
} from "./daemonAsks.ts";
import type { DaemonEvent } from "../types";

/** The permission asks agents have open on `hosts`: one `session.list` per
 * host when the page opens (and again on `session.resync`), then the
 * `session.ask` / `session.askClosed` stream. Events that arrive while a list
 * is in flight are replayed on top of it. */
export function useDaemonAsks(hosts: string[]): AsksByHost {
  const [asks, setAsks] = useState<AsksByHost>(noAsks);
  const signature = [...new Set(hosts)].sort().join("\n");
  useEffect(() => {
    const bridge = window.bridge;
    if (!bridge || !signature) return;
    const watched = new Set(signature.split("\n"));
    let stopped = false;
    const pending: Record<string, DaemonEvent[] | undefined> = {};
    const list = (host: string) => {
      const kept: DaemonEvent[] = [];
      pending[host] = kept;
      bridge
        .sessionsList(host)
        .then((sessions) => {
          if (stopped) return;
          if (pending[host] === kept) pending[host] = undefined;
          setAsks((state) =>
            kept.reduce(
              (next, event) => applyAskEvent(next, event),
              applyAskList(state, host, sessions),
            ),
          );
        })
        .catch(() => {
          if (pending[host] === kept) pending[host] = undefined;
        });
    };
    const off = bridge.onDaemonEvent((event) => {
      if (!watched.has(event.host)) return;
      if (event.method === "session.resync") return list(event.host);
      pending[event.host]?.push(event);
      setAsks((state) => applyAskEvent(state, event));
    });
    for (const host of watched) list(host);
    return () => {
      stopped = true;
      off();
    };
  }, [signature]);
  return asks;
}
