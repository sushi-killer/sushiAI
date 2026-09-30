import { useCallback, useState } from "react";
import type { Layout } from "../types";
import {
  MAX_AGENT_TABS,
  type AgentFocus,
  type AgentTab,
  type Saved,
} from "../workspaceState.ts";

const noFocus: AgentFocus = { providerId: "", agentId: "", active: "" };

/** The small pieces of session state that used to keep their own localStorage
 * key each - merged-project canvases, the Agent tabs and the focus of Agent
 * and Chat. App owns them so they travel in the one workspace snapshot
 * (`useAppPersistence`); the views get their starting values and report
 * changes through the setters. */
export function useSessionState(saved: Saved | null) {
  const [mergedLayouts, setMergedLayouts] = useState<Record<string, Layout>>(
    saved?.mergedLayouts ?? {},
  );
  const [agentTabs, setTabs] = useState<AgentTab[]>(saved?.agentTabs ?? []);
  const [agentFocus, setFocus] = useState<AgentFocus>(
    saved?.agentFocus ?? noFocus,
  );
  const [chatFocus, setChatFocus] = useState(saved?.chatFocus ?? "");
  const setAgentTabs = useCallback(
    (update: React.SetStateAction<AgentTab[]>) =>
      setTabs((old) =>
        (typeof update === "function" ? update(old) : update).slice(
          -MAX_AGENT_TABS,
        ),
      ),
    [],
  );
  const setAgentFocus = useCallback(
    (next: AgentFocus) =>
      setFocus((old) =>
        old.providerId === next.providerId &&
        old.agentId === next.agentId &&
        old.active === next.active
          ? old
          : next,
      ),
    [],
  );
  return {
    mergedLayouts,
    setMergedLayouts,
    agentTabs,
    setAgentTabs,
    agentFocus,
    setAgentFocus,
    chatFocus,
    setChatFocus,
  };
}

export type SessionState = ReturnType<typeof useSessionState>;
