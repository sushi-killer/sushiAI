import type { Workspace } from "../types";
import { groupKey } from "./workspaceMerge.ts";

type WorkspaceKey = Pick<Workspace, "id" | "cwd" | "connection">;

export function projectGitRequestKey(workspace: WorkspaceKey): string {
  return JSON.stringify([
    workspace.id,
    workspace.connection ?? "",
    workspace.cwd,
  ]);
}

export function readyProjectGitWorkspaceIds(
  workspaces: WorkspaceKey[],
  settledKeys: Record<string, string>,
): Set<string> {
  return new Set(
    workspaces
      .filter(
        (workspace) =>
          !workspace.cwd ||
          settledKeys[workspace.id] === projectGitRequestKey(workspace),
      )
      .map((workspace) => workspace.id),
  );
}

export function readyProjectGitHostKeys(
  workspaces: WorkspaceKey[],
  settledKeys: Record<string, string>,
): Set<string> {
  const readyIds = readyProjectGitWorkspaceIds(workspaces, settledKeys);
  const byHost = new Map<string, string[]>();
  for (const workspace of workspaces) {
    const host = groupKey(workspace.connection);
    byHost.set(host, [...(byHost.get(host) ?? []), workspace.id]);
  }
  return new Set(
    [...byHost]
      .filter(([, ids]) => ids.every((id) => readyIds.has(id)))
      .map(([host]) => host),
  );
}
