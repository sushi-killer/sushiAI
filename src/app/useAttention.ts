import { useCallback, useEffect, useRef, useState } from "react";
import {
  createAttentionState,
  dueReminders,
  inboxGroups,
  markSeen,
  observe,
  endReminders,
  isPanelOnScreen,
  reminderNotice,
  transitionNotice,
  waitingCount,
  workingCount,
  type AttentionState,
} from "./attention.ts";
import { groupKey } from "../lib/hostGroup.ts";
import { groupLabel } from "./workspaceMerge.ts";
import { moduleUis } from "../extensions/modules.ts";
import type { ModuleEntry } from "./inboxModel.ts";
import type { SectionRef } from "./navigation.ts";
import type { ConnectionProfile, Workspace } from "../types";

/** How often the 5/10/20 minute and then 30 minute reminders are checked - independent of
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
  mode,
  drawnPanelIds,
  selected,
  zoomed,
  switchWorkspace,
  revealPane,
}: {
  workspaces: Workspace[];
  connectionProfiles: ConnectionProfile[];
  /** No section page is open - only then can a pane actually be on screen. */
  section: SectionRef | null;
  /** The Code canvas is only drawn in Code mode. */
  mode: string;
  /** The panes the canvas draws now (see `drawnPanelIds`). */
  drawnPanelIds: string[];
  selected: string;
  zoomed: string | null;
  switchWorkspace(id: string): void;
  revealPane(workspaceId: string, panelId: string): void;
}) {
  // Modules put rows in the Inbox. `moduleUis` is a constant list, so the
  // hooks below run in the same order on every render.
  const moduleItems: ModuleEntry[] = moduleUis.flatMap((ui) =>
    ui.useAttention().map((item) => ({ extensionId: ui.extensionId, item })),
  );
  const [state, setState] = useState<AttentionState>(createAttentionState);
  const stateRef = useRef(state);
  stateRef.current = state;
  const workspacesRef = useRef(workspaces);
  workspacesRef.current = workspaces;
  const profilesRef = useRef(connectionProfiles);
  profilesRef.current = connectionProfiles;
  const viewRef = useRef({ mode, section, zoomed, selected, drawnPanelIds });
  viewRef.current = { mode, section, zoomed, selected, drawnPanelIds };

  /** Read at send time (events and timers fire outside a render) so main can
   * stay quiet only for a pane the owner is already looking at. */
  const onScreen = useCallback(
    (panelId: string) => isPanelOnScreen({ panelId, ...viewRef.current }),
    [],
  );

  const hostLabelFor = useCallback(
    (workspace: Workspace) =>
      groupLabel(groupKey(workspace.connection), connectionProfiles),
    [connectionProfiles],
  );

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
    for (const event of events)
      void window.bridge?.attentionNotify(
        transitionNotice(event, hostLabelFor(event.workspace), onScreen),
      );
  }, [workspaces, hostLabelFor, onScreen]);

  useEffect(() => {
    const timer = setInterval(() => {
      // Looking at a waiting pane ends its reminders. The window must also
      // be focused: a pane left selected in a background window is unseen.
      let current = stateRef.current;
      if (document.hasFocus())
        for (const id of Object.keys(current.panels))
          if (onScreen(id)) current = endReminders(current, id);
      const { reminders, state: next } = dueReminders(current, Date.now());
      if (next !== stateRef.current) setState(next);
      const notice = reminderNotice(
        reminders,
        next,
        workspacesRef.current,
        profilesRef.current,
        onScreen,
      );
      if (notice) void window.bridge?.attentionNotify(notice);
    }, REMINDER_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [onScreen]);

  const waiting = waitingCount(
    workspaces,
    state,
    connectionProfiles,
    moduleItems.map((entry) => entry.item),
  );
  const working = workingCount(workspaces, state, connectionProfiles);
  const lastBadge = useRef("");
  useEffect(() => {
    const badge = `${waiting}:${working}`;
    if (lastBadge.current === badge) return;
    lastBadge.current = badge;
    void window.bridge?.attentionBadge(waiting, working);
  }, [waiting, working]);

  // Seen the moment it can actually be seen (see `isPanelOnScreen`).
  const drawnKey = drawnPanelIds.join("\n");
  useEffect(() => {
    for (const id of [zoomed, selected])
      if (id && onScreen(id)) setState((current) => markSeen(current, id));
  }, [mode, section, zoomed, selected, drawnKey, onScreen]);

  useEffect(
    () =>
      window.bridge?.onAttentionOpen((target) => {
        switchWorkspace(target.workspaceId);
        revealPane(target.workspaceId, target.panelId);
      }),
    [switchWorkspace, revealPane],
  );

  return {
    groups: inboxGroups(workspaces, state, connectionProfiles),
    moduleItems,
    waiting,
    markSeen: (panelId: string) =>
      setState((current) => markSeen(current, panelId)),
  };
}
