import { herdrWorkspaceKey } from "../herdrIdentity.ts";
import { appendPanel, reopenInSlot } from "./workspace-actions.ts";
import type { Panel, SessionLaunchValue, Workspace } from "../types";

export function applySessionLaunch(
  workspaces: Workspace[],
  endpoint: string,
  value: SessionLaunchValue,
  panel: Panel,
  restore?: { workspaceId: string; panelId: string },
  name?: string,
): Workspace[] {
  const workspaceId = herdrWorkspaceKey(endpoint, value.workspaceId);
  const listed = workspaces.find(
    (workspace) =>
      workspace.connection === endpoint &&
      workspace.herdrId === value.workspaceId,
  );
  const owner =
    restore &&
    workspaces.find((workspace) => workspace.id === restore.workspaceId);
  let next: Workspace;
  if (owner && restore) {
    next = {
      ...reopenInSlot(owner, restore.panelId, panel, value.workspaceId),
      id: owner.id,
      cwd: value.cwd,
      connection: endpoint,
    };
    if (listed && listed !== owner)
      for (const item of listed.panels)
        if (!next.panels.some((existing) => existing.id === item.id))
          next = appendPanel(next, item);
  } else if (listed) {
    next = listed.panels.some((item) => item.id === panel.id)
      ? {
          ...listed,
          panels: listed.panels.map((item) =>
            item.id === panel.id ? { ...item, ...panel } : item,
          ),
        }
      : appendPanel(listed, panel);
  } else {
    next = appendPanel(
      {
        id: workspaceId,
        herdrId: value.workspaceId,
        connection: endpoint,
        name: name || value.cwd.split("/").filter(Boolean).pop() || value.cwd,
        cwd: value.cwd,
        panels: [],
        layout: null,
      },
      panel,
    );
  }
  const replaced = new Set(
    [listed?.id, owner?.id].filter((id): id is string => !!id),
  );
  if (!replaced.size) return [...workspaces, next];
  let inserted = false;
  return workspaces.flatMap((workspace) => {
    if (!replaced.has(workspace.id)) return [workspace];
    if (inserted) return [];
    inserted = true;
    return [next];
  });
}
