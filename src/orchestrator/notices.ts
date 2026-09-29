// Pure logic behind the orchd task notices: what a toast stack does with a
// notice, and which workspace a notice opens. No React, no bridge.
import { contains } from "../layout.ts";
import type { Workspace } from "../types.ts";

export type TaskNoticeFocus = "question" | "summary" | "report";

/** What the main process sends (electron/orchestrator.cjs `orchestratorNotice`). */
export type TaskNotice = {
  taskId: string;
  repo: string;
  kind: "input" | "done" | "failed";
  title: string;
  body: string;
  focus: TaskNoticeFocus;
};

/** What opening a notice needs: the task, its repo and where to scroll. */
export type TaskTarget = {
  taskId: string;
  repo: string;
  focus: TaskNoticeFocus;
};

export const MAX_TOASTS = 3;
export const TIMED_TOAST_MS = 8000;

export type Toast = TaskNotice & { id: string };

export type ToastAction =
  | { type: "add"; notice: TaskNotice }
  | { type: "dismiss"; id: string }
  | { type: "task"; taskId: string; status: string };

/** How long a toast stays: a needs-input one until the owner acts (null),
 * a done/failed one for a few seconds. */
export function toastLifetimeMs(
  notice: Pick<TaskNotice, "kind">,
): number | null {
  return notice.kind === "input" ? null : TIMED_TOAST_MS;
}

export function toastId(notice: TaskNotice): string {
  return `${notice.kind}:${notice.taskId}:${notice.body}`;
}

export function toastReducer(state: Toast[], action: ToastAction): Toast[] {
  switch (action.type) {
    case "add": {
      const id = toastId(action.notice);
      const rest = state.filter((toast) => toast.id !== id);
      return [...rest, { ...action.notice, id }].slice(-MAX_TOASTS);
    }
    case "dismiss":
      return state.filter((toast) => toast.id !== action.id);
    case "task":
      // A question that is no longer being asked has nothing left to answer.
      return action.status === "waiting"
        ? state
        : state.filter(
            (toast) =>
              !(toast.kind === "input" && toast.taskId === action.taskId),
          );
  }
}

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
