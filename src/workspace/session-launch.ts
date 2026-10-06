import { herdrWorkspaceKey } from "../herdrIdentity.ts";
import { appendPanel, isVanished, reopenInSlot } from "./workspace-actions.ts";
import type {
  DaemonLaunchRequest,
  Panel,
  SessionLaunchRequest,
  SessionLaunchValue,
  Workspace,
} from "../types";

/** The daemon host a launch endpoint belongs to. */
export const daemonHost = (endpoint: string) =>
  endpoint.startsWith("ssh:") ? endpoint : "local";

/** One daemon launch from a UI launch request. `operationId` is the
 * idempotency key: the renderer makes one per launch and reuses it on retry. */
export function toDaemonLaunch(
  request: SessionLaunchRequest,
  size = { cols: 120, rows: 32 },
): DaemonLaunchRequest {
  return {
    host: daemonHost(request.endpoint),
    cwd: request.cwd,
    ...size,
    ...(request.kind === "agent" && request.agent
      ? { agent: request.agent }
      : {}),
    title: request.label,
    ...(request.prompt ? { prompt: request.prompt } : {}),
    ...(request.workspaceId ? { group: request.workspaceId } : {}),
    ...(request.claudeAccountId !== undefined
      ? { claudeAccountId: request.claudeAccountId }
      : {}),
    ...(request.codexAccountId !== undefined
      ? { codexAccountId: request.codexAccountId }
      : {}),
    ...(request.modelProfileId
      ? { modelProfileId: request.modelProfileId }
      : {}),
    ...(request.worktree ? { worktree: request.worktree } : {}),
    idempotencyKey: request.operationId,
  };
}

/** Launches through the daemon and returns the panel to place: the template
 * bound to the new session. A failure throws; retry with the same request. */
export async function launchDaemonSession(
  bridge: {
    daemonSessionLaunch(
      request: DaemonLaunchRequest,
    ): Promise<{ host: string; sessionId: string }>;
  },
  request: SessionLaunchRequest,
  template: Panel,
): Promise<{ host: string; sessionId: string; panel: Panel }> {
  const { host, sessionId } = await bridge.daemonSessionLaunch(
    toDaemonLaunch(request),
  );
  const panel: Panel = { ...template, sessionId };
  delete panel.ended;
  return { host, sessionId, panel };
}

export function findSessionWorkspace(
  workspaces: Workspace[],
  endpoint: string,
  value: SessionLaunchValue,
): Workspace | undefined {
  return (
    workspaces.find(
      (workspace) =>
        workspace.connection === endpoint &&
        workspace.herdrId === value.workspaceId,
    ) ||
    workspaces.find(
      (workspace) =>
        workspace.connection === endpoint &&
        workspace.cwd === value.cwd &&
        isVanished(workspace),
    )
  );
}

export function applySessionLaunch(
  workspaces: Workspace[],
  endpoint: string,
  value: SessionLaunchValue,
  panel: Panel,
  restore?: { workspaceId: string; panelId: string },
  name?: string,
  worktreeBranch?: string,
): Workspace[] {
  const workspaceId = herdrWorkspaceKey(endpoint, value.workspaceId);
  const listed = findSessionWorkspace(workspaces, endpoint, value);
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
    const updated = listed.panels.some((item) => item.id === panel.id)
      ? {
          ...listed,
          panels: listed.panels.map((item) =>
            item.id === panel.id ? { ...item, ...panel } : item,
          ),
        }
      : appendPanel(listed, panel);
    next = { ...updated, herdrId: value.workspaceId, cwd: value.cwd };
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
  if (worktreeBranch) next = { ...next, worktreeBranch };
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
