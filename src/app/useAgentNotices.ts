import { useEffect, useState } from "react";
import type { AgentActivity } from "../agents/types";

/** The newest hundred agent activities, newest first, one entry per activity id
 * so a re-reported activity moves up instead of appearing twice. */
export function useAgentNotices(notify: (text: string) => void) {
  const [notices, setNotices] = useState<AgentActivity[]>([]);
  useEffect(
    () =>
      window.bridge?.onAgents((event) => {
        if (event.type !== "activity") return;
        const activity = event.activity as AgentActivity;
        setNotices((old) =>
          [activity, ...old.filter((item) => item.id !== activity.id)].slice(
            0,
            100,
          ),
        );
        notify(`${activity.agentName} · ${activity.title}`);
      }),
    [notify],
  );
  return { notices, setNotices };
}
