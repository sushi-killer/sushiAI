// Pure logic behind the orchd task notices: which workspace a notice opens. No React, no bridge.
import { contains } from "../layout.ts";
import { elapsedLabel, projectName } from "./ownerAttention.ts";
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
  host?: string;
  /** Epoch ms the task last moved (a question: when it was asked). */
  at?: number;
  /** The repo's folder name, for the bubble header. */
  repoName?: string;
  costUsd?: number;
  /** The last review's verdict, when a review ran. */
  verdict?: "PASS" | "FAIL";
  /** A done task whose branch already landed. */
  landed?: boolean;
};

/** What opening a notice needs: the task, its repo and where to scroll. */
export type TaskTarget = {
  taskId: string;
  repo: string;
  focus: TaskNoticeFocus;
  host?: string;
};

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
        item.kind === "orchestrator" && contains(workspace.layout, item.id),
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
        item.kind === "orchestrator" && contains(workspace.layout, item.id),
    );
    if (panel)
      return { kind: "panel", workspaceId: workspace.id, panelId: panel.id };
  }
  if (matching.length)
    return { kind: "add-panel", workspaceId: matching[0].id };
  return { kind: "create-workspace", name: projectName(repo), cwd: repo };
}

const TAG_LABELS: Record<string, string> = {
  input: "Needs you",
  done: "Done",
  failed: "Failed",
  stopped: "Stopped",
  landing: "Landing",
};

/** "now", "5m", "3h", "2d": how long ago `at` was. */
export function noticeAge(at: number, now: number): string {
  const ms = Math.max(0, now - at);
  return ms < 60_000 ? "now" : elapsedLabel(ms);
}

/** The bubble header: the state tag and "repo · age" (each part only when the
 * notice carries it). */
export function noticeHeader(
  notice: Pick<TaskNotice, "kind" | "repo" | "repoName" | "at">,
  now: number,
): { label: string; source: string } {
  const repo = notice.repoName || projectName(notice.repo);
  const parts = [repo, notice.at ? noticeAge(notice.at, now) : ""];
  return {
    label: TAG_LABELS[notice.kind] ?? "Notice",
    source: parts.filter(Boolean).join(" · "),
  };
}

/** The done bubble's meta line: "$0.31 · review PASS · not landed". Null when
 * the notice carries no cost, so the caller falls back to its body. */
export function doneMeta(
  notice: Pick<TaskNotice, "costUsd" | "verdict" | "landed">,
): string | null {
  if (typeof notice.costUsd !== "number") return null;
  return [
    `$${notice.costUsd.toFixed(2)}`,
    notice.verdict ? `review ${notice.verdict}` : "",
    notice.landed ? "landed" : "not landed",
  ]
    .filter(Boolean)
    .join(" · ");
}
