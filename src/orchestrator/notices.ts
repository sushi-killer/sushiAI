// Pure logic behind the orchd task notices: which workspace a notice opens. No React, no bridge.
import { contains } from "../layout.ts";
import type { Workspace } from "../types.ts";

export type TaskNoticeFocus = "question" | "summary" | "report";

/** What the main process sends (electron/orchestrator.cjs `orchestratorNotice`). */
export type TaskNotice = {
  taskId: string;
  repo: string;
  kind: "input" | "done" | "failed" | "landing" | "stopped";
  title: string;
  body: string;
  focus: TaskNoticeFocus;
  /** Answer choices of a needs-input notice. */
  options?: string[];
  /** A done task that is not landed: the notice offers a Land action. */
  canLand?: boolean;
};

/** What opening a notice needs: the task, its repo and where to scroll. */
export type TaskTarget = {
  taskId: string;
  repo: string;
  focus: TaskNoticeFocus;
};

export type OrchestratorTarget =
  | { kind: "panel"; workspaceId: string; panelId: string }
  | { kind: "add-panel"; workspaceId: string }
  | { kind: "create-workspace"; name: string; cwd: string };

function baseName(repo: string): string {
  const parts = repo.split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] || repo;
}

/** The place a notice for `repo` opens: the Orchestrator panel of the
 * workspace whose cwd is the repo, else that workspace (a panel to add), else
 * a workspace to create. A workspace that already shows an Orchestrator panel
 * wins over one that does not. */
export function orchestratorTarget(
  workspaces: Workspace[],
  repo: string,
): OrchestratorTarget {
  const matching = workspaces.filter((workspace) => workspace.cwd === repo);
  for (const workspace of matching) {
    const panel = workspace.panels.find(
      (item) =>
        item.kind === "orchestrator" && contains(workspace.layout, item.id),
    );
    if (panel)
      return { kind: "panel", workspaceId: workspace.id, panelId: panel.id };
  }
  if (matching.length)
    return { kind: "add-panel", workspaceId: matching[0].id };
  return { kind: "create-workspace", name: baseName(repo), cwd: repo };
}
