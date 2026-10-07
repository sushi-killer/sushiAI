import type { Panel, Workspace } from "../types";
import { groupKey } from "../lib/hostGroup.ts";
import type { ProjectGit } from "./useProjectGit";

export type WorktreeCleanupTarget = {
  checkout: string;
  branch: string;
  blocked: boolean;
};

export type WorktreeCleanupRequest = WorktreeCleanupTarget & {
  workspace: Workspace;
  panel: Panel;
};

export type WorktreeDeleteChoice = {
  requestKey: string;
  checked: boolean;
  manual: boolean;
};

export function worktreeCleanupRequestKey(
  connection: string | undefined,
  target: WorktreeCleanupTarget | null,
): string {
  return target
    ? JSON.stringify([
        connection ?? "",
        target.checkout,
        target.branch,
        target.blocked,
      ])
    : "";
}

export function isWorktreeCleanupSelected(
  choice: WorktreeDeleteChoice,
  requestKey: string,
): boolean {
  return (
    Boolean(requestKey) && choice.requestKey === requestKey && choice.checked
  );
}

function isInside(checkout: string, cwd: string): boolean {
  const root = checkout.replace(/\/+$/, "");
  return cwd === root || cwd.startsWith(`${root}/`);
}

export function worktreeCleanupTarget(
  workspace: Workspace,
  panel: Panel,
  workspaces: Workspace[],
  projectGit: Record<string, ProjectGit>,
): WorktreeCleanupTarget | null {
  const git = projectGit[workspace.id];
  if (!git?.checkout || !git.commonDir || !git.branch) return null;
  if (!git.linkedWorktree) return null;

  const blocked = workspaces.some((candidate) => {
    if (groupKey(candidate.connection) !== groupKey(workspace.connection))
      return false;
    const candidateCheckout = projectGit[candidate.id]?.checkout;
    const sameCheckout = candidateCheckout
      ? candidateCheckout === git.checkout
      : isInside(git.checkout, candidate.cwd);
    return (
      sameCheckout && candidate.panels.some((item) => item.id !== panel.id)
    );
  });

  return { checkout: git.checkout, branch: git.branch, blocked };
}

export async function closeBeforeWorktreeRemoval(
  closeSession: () => Promise<void>,
  removeWorktree?: () => Promise<unknown>,
): Promise<
  | { closed: false; closeError: unknown }
  | { closed: true; cleanupError?: unknown }
> {
  try {
    await closeSession();
  } catch (closeError) {
    return { closed: false, closeError };
  }
  if (!removeWorktree) return { closed: true };
  try {
    await removeWorktree();
    return { closed: true };
  } catch (cleanupError) {
    return { closed: true, cleanupError };
  }
}

/** `gh` reports a merged pull request as MERGED (OPEN and CLOSED are the
 * other two states); a closed one that never merged is not. */
export function isMergedPullRequest(value: unknown): boolean {
  return (
    !!value &&
    typeof value === "object" &&
    (value as { state?: unknown }).state === "MERGED"
  );
}
