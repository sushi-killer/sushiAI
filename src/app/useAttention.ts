import { useCallback, useEffect, useRef, useState } from "react";
import {
  createAttentionState,
  dueReminders,
  inboxGroups,
  markSeen,
  observe,
  waitingCount,
  type AttentionState,
} from "./attention.ts";
import { groupKey, groupLabel } from "./workspaceMerge.ts";
import type { SectionRef } from "./navigation.ts";
import type { ConnectionProfile, Workspace } from "../types";

/** How often the 5/10/20 minute reminders are checked - independent of
 * whatever else re-renders the app, so a host that stopped polling still gets
 * its nudge on schedule. */
const REMINDER_INTERVAL_MS = 30000;

/** The attention queue: tracks agent panels across every workspace, notifies
 * on a real transition or a due reminder, keeps the Dock badge in step with
 * how many things are waiting, and marks a panel seen the moment the user is
 * actually looking at it. Called once, in `App`. */
export function useAttention({
  workspaces,
  connectionProfiles,
  section,
  active,
  selected,
  zoomed,
  switchWorkspace,
  setSelected,
  setZoomed,
}: {
  workspaces: Workspace[];
  connectionProfiles: ConnectionProfile[];
  /** No section page is open - only then can a pane actually be on screen. */
  section: SectionRef | null;
  active: Workspace;
  selected: string;
  zoomed: string | null;
  switchWorkspace(id: string): void;
  setSelected(id: string): void;
  setZoomed(id: string | null): void;
}) {
  const [state, setState] = useState<AttentionState>(createAttentionState);
  const stateRef = useRef(state);
  stateRef.current = state;
  const workspacesRef = useRef(workspaces);
  workspacesRef.current = workspaces;

  const hostLabelFor = useCallback(
    (workspace: Workspace) =>
      groupLabel(groupKey(workspace.connection), connectionProfiles),
    [connectionProfiles],
  );

  const findPanel = useCallback((panelId: string) => {
    for (const workspace of workspacesRef.current) {
      const panel = workspace.panels.find((p) => p.id === panelId);
      if (panel) return { workspace, panel };
    }
    return null;
  }, []);

  // Runs whenever the workspace list changes - a panel entering `blocked` or
  // `done` notifies the same render the transition is seen. Reads the latest
  // tracked state through the ref rather than depending on it, or applying
  // the very transitions this effect just recorded would re-fire them next
  // render.
  useEffect(() => {
    const { state: next, events } = observe(
      stateRef.current,
      workspaces,
      Date.now(),
    );
    setState(next);
    for (const event of events) {
      const hostLabel = hostLabelFor(event.workspace);
      const notice =
        event.kind === "blocked"
          ? {
              title: `${event.panel.title} needs your input`,
              body: `${event.workspace.name} · ${hostLabel}`,
            }
          : {
              title: `${event.panel.title} finished`,
              body: `${event.workspace.name} · ${hostLabel}`,
            };
      void window.bridge?.attentionNotify({
        workspaceId: event.workspace.id,
        panelId: event.panel.id,
        ...notice,
      });
    }
  }, [workspaces, hostLabelFor]);

  useEffect(() => {
    const timer = setInterval(() => {
      const { reminders, state: next } = dueReminders(
        stateRef.current,
        Date.now(),
      );
      if (!reminders.length) return;
      setState(next);
      for (const reminder of reminders) {
        const found = findPanel(reminder.panelId);
        if (!found) continue;
        void window.bridge?.attentionNotify({
          workspaceId: found.workspace.id,
          panelId: found.panel.id,
          title: `${found.panel.title} is still waiting`,
          body: `${reminder.minutes} min · ${found.workspace.name}`,
        });
      }
    }, REMINDER_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [findPanel]);

  const waiting = waitingCount(workspaces, state);
  const lastBadge = useRef(-1);
  useEffect(() => {
    if (lastBadge.current === waiting) return;
    lastBadge.current = waiting;
    void window.bridge?.attentionBadge(waiting);
  }, [waiting]);

  // Seen the moment it can actually be seen: the active workspace, no section
  // page covering the canvas, and this is the selected or zoomed pane.
  useEffect(() => {
    if (section) return;
    const looked = zoomed || selected;
    if (!looked) return;
    if (!active.panels.some((p) => p.id === looked)) return;
    setState((current) => markSeen(current, looked));
  }, [section, zoomed, selected, active]);

  useEffect(
    () =>
      window.bridge?.onAttentionOpen((target) => {
        switchWorkspace(target.workspaceId);
        setSelected(target.panelId);
        setZoomed(target.panelId);
      }),
    [switchWorkspace, setSelected, setZoomed],
  );

  return {
    groups: inboxGroups(workspaces, state, connectionProfiles),
    waiting,
    markSeen: (panelId: string) =>
      setState((current) => markSeen(current, panelId)),
  };
}
