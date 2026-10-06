// Pure logic behind the orchd task notices: which workspace a notice opens. No React, no bridge.
import { contains } from "../layout.ts";
import { projectName } from "./ownerAttention.ts";
import type { Panel, Workspace } from "../types.ts";

export type TaskNoticeFocus = "question" | "summary" | "report";

/** What opening a notice needs: the task, its repo and where to scroll. */
export type TaskTarget = {
  taskId: string;
  repo: string;
  focus: TaskNoticeFocus;
  host?: string;
};

/** The surface id of the orchestrator pane in the extension manifest. The
 * extension id literal is `enabled.ts`'s ORCHESTRATOR_EXTENSION_ID, kept here
 * as text because that module pulls in React state. */
export const ORCHESTRATION_SURFACE = "orchestration";

/** A pane of the orchestrator's `orchestration` surface. */
function isOrchestrationPane(panel: Panel): boolean {
  return (
    panel.kind === "extension" &&
    panel.extension.extensionId === "builtin.orchestrator" &&
    panel.extension.contributionId === ORCHESTRATION_SURFACE
  );
}

export type OrchestratorTarget =
  | { kind: "panel"; workspaceId: string; panelId: string }
  | { kind: "add-panel"; workspaceId: string }
  | { kind: "create-workspace"; name: string; cwd: string };

function remoteTarget(
  workspaces: Workspace[],
  repo: string,
  host: string,
): OrchestratorTarget {
  const onHost = workspaces.filter(
    (workspace) => workspace.connection === host,
  );
  const ordered = [
    ...onHost.filter((workspace) => workspace.cwd === repo),
    ...onHost,
    ...workspaces,
  ];
  for (const workspace of ordered) {
    const panel = workspace.panels.find(
      (item) =>
        isOrchestrationPane(item) && contains(workspace.layout, item.id),
    );
    if (panel)
      return { kind: "panel", workspaceId: workspace.id, panelId: panel.id };
  }
  if (ordered.length) return { kind: "add-panel", workspaceId: ordered[0].id };
  return { kind: "create-workspace", name: projectName(repo), cwd: repo };
}

/** The place a notice for `repo` opens: the Orchestrator panel of the
 * workspace whose cwd is the repo, else that workspace (a panel to add), else
 * a workspace to create. A workspace that already shows an Orchestrator panel
 * wins over one that does not. */
export function orchestratorTarget(
  workspaces: Workspace[],
  repo: string,
  host?: string,
): OrchestratorTarget {
  if (host) return remoteTarget(workspaces, repo, host);
  const matching = workspaces.filter((workspace) => workspace.cwd === repo);
  for (const workspace of matching) {
    const panel = workspace.panels.find(
      (item) =>
        isOrchestrationPane(item) && contains(workspace.layout, item.id),
    );
    if (panel)
      return { kind: "panel", workspaceId: workspace.id, panelId: panel.id };
  }
  if (matching.length)
    return { kind: "add-panel", workspaceId: matching[0].id };
  return { kind: "create-workspace", name: projectName(repo), cwd: repo };
}
