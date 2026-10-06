import { daemonHost, sessionPanelId } from "../daemonSessions.ts";
import { appendPanel, reopenInSlot } from "./workspace-actions.ts";
import { leaf } from "../layout.ts";
import type {
  DaemonLaunchRequest,
  Panel,
  SessionLaunchRequest,
  Workspace,
} from "../types";

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
    ...(request.resume ? { resume: request.resume } : {}),
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
 * bound to the new session, with the panel id every other path derives for it.
 * A failure throws; retry with the same request (same `operationId`). */
export async function launchDaemonSession(
  bridge: {
    daemonSessionLaunch(
      request: DaemonLaunchRequest,
    ): Promise<{ host: string; sessionId: string; cwd?: string }>;
  },
  request: SessionLaunchRequest,
  template: Panel,
): Promise<{ host: string; sessionId: string; cwd: string; panel: Panel }> {
  const { host, sessionId, cwd } = await bridge.daemonSessionLaunch(
    toDaemonLaunch(request),
  );
  const panel: Panel = {
    ...template,
    id: sessionPanelId(request.endpoint, sessionId),
    sessionId,
  };
  delete panel.ended;
  return { host, sessionId, cwd: cwd || request.cwd, panel };
}

/** The existing workspace a launch lands in, or undefined when it needs a new
 * one: a Reopen goes back to its workspace, a worktree launch gets a workspace
 * of its own, a new panel joins the workspace that asked for it. */
export function launchTarget(
  workspaces: Workspace[],
  request: SessionLaunchRequest,
  panelId: string,
  restore?: { workspaceId: string },
): string | undefined {
  // A replayed launch (same key, same session) lands where it already is.
  const holder = restore
    ? undefined
    : workspaces.find((workspace) =>
        workspace.panels.some((panel) => panel.id === panelId),
      );
  const id =
    holder?.id ??
    restore?.workspaceId ??
    (request.worktree ? "" : request.workspaceId);
  return workspaces.find((workspace) => workspace.id === id)?.id;
}

/** Places the launched panel: into `targetId` when that workspace exists
 * (Reopen takes the ended panel's slot), else into a new workspace with that
 * id on the launch's folder. */
export function placeLaunchedPanel(
  workspaces: Workspace[],
  {
    targetId,
    request,
    panel,
    cwd,
    restore,
  }: {
    targetId: string;
    request: SessionLaunchRequest;
    panel: Panel;
    cwd: string;
    restore?: { panelId: string };
  },
): Workspace[] {
  const target =
    (!restore &&
      workspaces.find((workspace) =>
        workspace.panels.some((item) => item.id === panel.id),
      )) ||
    workspaces.find((workspace) => workspace.id === targetId);
  if (!target)
    return [
      ...workspaces,
      {
        id: targetId,
        name: request.label || cwd.split("/").filter(Boolean).pop() || cwd,
        cwd,
        connection: request.endpoint,
        ...(request.worktree
          ? { worktreeBranch: request.worktree.branch }
          : {}),
        panels: [panel],
        layout: leaf(panel.id),
      },
    ];
  const next = restore
    ? reopenInSlot(target, restore.panelId, panel)
    : target.panels.some((item) => item.id === panel.id)
      ? {
          ...target,
          panels: target.panels.map((item) =>
            item.id === panel.id ? { ...item, ...panel } : item,
          ),
        }
      : appendPanel(target, panel);
  return workspaces.map((workspace) =>
    workspace.id === target.id ? next : workspace,
  );
}
