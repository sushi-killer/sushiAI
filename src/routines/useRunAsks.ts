import { useEffect, useState } from "react";
import type { DaemonAsk } from "../types.ts";

/** One owner question of a run, as the daemon holds it. */
export type OwnerQuestion = {
  askId: string;
  session: string;
  text: string;
  options: { id: string; label: string }[];
};

const OWNER_TOOL = "owner.ask";

/** The question an ask carries (`sushiai ask`), or null for a permission ask. */
export function ownerQuestion(ask: DaemonAsk): OwnerQuestion | null {
  if (ask.tool !== OWNER_TOOL || !ask.input || typeof ask.input !== "object")
    return null;
  const input = ask.input as { text?: unknown; options?: unknown };
  const options = Array.isArray(input.options)
    ? input.options.flatMap((option) =>
        option &&
        typeof option === "object" &&
        typeof (option as { id?: unknown }).id === "string"
          ? [
              {
                id: (option as { id: string }).id,
                label: String(
                  (option as { label?: unknown }).label ??
                    (option as { id: string }).id,
                ),
              },
            ]
          : [],
      )
    : [];
  return {
    askId: ask.askId,
    session: ask.session,
    text: typeof input.text === "string" ? input.text : "",
    options,
  };
}

/** The open `sushiai ask` questions of one run session, kept live from the
 * daemon's own events. `host` is the daemon IPC host key. */
export function useRunAsks(
  host: string | null,
  sessionId: string | undefined,
): OwnerQuestion[] {
  const [asks, setAsks] = useState<OwnerQuestion[]>([]);
  useEffect(() => {
    setAsks([]);
    if (!host || !sessionId || !window.bridge) return;
    let live = true;
    void window.bridge
      .sessionsList(host)
      .then((sessions) => {
        if (!live) return;
        const session = sessions.find((item) => item.id === sessionId);
        setAsks(
          (session?.asks ?? []).flatMap((ask) => {
            const question = ownerQuestion(ask);
            return question ? [question] : [];
          }),
        );
      })
      .catch(() => {});
    const off = window.bridge.onDaemonEvent((event) => {
      if (event.host !== host) return;
      if (event.method === "session.ask") {
        const question = ownerQuestion(event.params as DaemonAsk);
        if (question && question.session === sessionId)
          setAsks((list) => [
            ...list.filter((item) => item.askId !== question.askId),
            question,
          ]);
      } else if (event.method === "session.askClosed") {
        const { askId } = event.params as { askId: string };
        setAsks((list) => list.filter((item) => item.askId !== askId));
      } else if (event.method === "session.exited") {
        if ((event.params as { id?: string }).id === sessionId) setAsks([]);
      }
    });
    return () => {
      live = false;
      off();
    };
  }, [host, sessionId]);
  return asks;
}
