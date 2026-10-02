import type { Workspace } from "../types.ts";

/** The branches of the worktrees the app launched sessions into that still
 * have a live panel. A closed session stays in its slot marked `ended`, so
 * only a panel that is not ended counts. A workspace's name is never read. */
export function liveWorktreeBranches(workspaces: Workspace[]): string[] {
  return workspaces
    .filter((w) => w.worktreeBranch && w.panels.some((p) => !p.ended))
    .map((w) => w.worktreeBranch!);
}
