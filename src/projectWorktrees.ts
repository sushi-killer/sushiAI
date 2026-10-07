import type {
  ConnectionProfile,
  Worktree,
  WorktreeList,
  Workspace,
} from "./types";
import type { WorktreeClaim } from "./extensions/modules";
import { claimFor } from "./lib/worktreeClaims.ts";

export type WorktreeRow = {
  key: string;
  host: string;
  hostLabel: string;
  /** The host's main checkout; removal lists the worktrees again from it,
   * which still works once the dialog's own worktree is gone. */
  cwd: string;
  /** The branch "merged" and "ahead" are measured against. */
  base: string;
  worktree: Worktree;
  /** Workspaces whose folder is this worktree or inside it. */
  open: Workspace[];
  claim?: WorktreeClaim;
};

/** "local" for this Mac (any non-ssh endpoint), else the ssh endpoint -
 * the host keys the worktree IPC uses. */
export function hostOf(connection: string | undefined): string {
  return connection?.startsWith("ssh:") ? connection : "local";
}

export function hostLabel(host: string, profiles: ConnectionProfile[]) {
  if (host === "local") return "This Mac";
  const profile = profiles.find((item) => `ssh:${item.id}` === host);
  return profile?.name || profile?.host || host.slice(4);
}

const inside = (folder: string, root: string) =>
  folder === root || folder.startsWith(root.endsWith("/") ? root : `${root}/`);

/** Every host's worktrees as one table: this Mac first, then hosts by name,
 * each host's main checkout before its linked worktrees. */
export function worktreeRows(
  lists: WorktreeList[],
  workspaces: Workspace[],
  claims: WorktreeClaim[],
  profiles: ConnectionProfile[],
): WorktreeRow[] {
  return lists
    .flatMap((list) =>
      "error" in list
        ? []
        : list.worktrees.map((worktree) => ({
            key: `${list.host}\0${worktree.path}`,
            host: list.host,
            hostLabel: hostLabel(list.host, profiles),
            cwd: list.root || list.cwd,
            base: list.base.replace(/^origin\//, "") || "main",
            worktree,
            open: workspaces.filter(
              (w) =>
                hostOf(w.connection) === list.host &&
                !!w.cwd &&
                inside(w.cwd, worktree.path),
            ),
            claim: claimFor(claims, list.host, {
              path: worktree.path,
              branch: worktree.branch,
              repo: list.root,
            }),
          })),
    )
    .sort((a, b) =>
      a.host === b.host
        ? 0
        : a.host === "local"
          ? -1
          : b.host === "local"
            ? 1
            : a.hostLabel.localeCompare(b.hostLabel),
    );
}

/** A row the trash button may act on: never the main checkout, a locked
 * worktree, or the checkout of a claim that is still in use. */
export function removable(row: WorktreeRow): boolean {
  return !row.worktree.main && !row.worktree.locked && !row.claim?.active;
}

/** What removing a row loses, in the order the confirmation says it. */
export function removalLoss(row: WorktreeRow): string {
  const { worktree, open, hostLabel, base } = row;
  const folder = worktree.path.split("/").pop();
  if (!worktree.exists)
    return `On ${hostLabel}, the folder ${folder} is already gone; the worktree is forgotten.`;
  const parts = [`On ${hostLabel}, the folder ${folder} is deleted`];
  if (worktree.changes)
    parts[0] += ` with its ${worktree.changes} uncommitted ${worktree.changes === 1 ? "change" : "changes"}`;
  const sessions = open.reduce((sum, w) => sum + w.panels.length, 0);
  if (sessions)
    parts.push(
      sessions === 1
        ? "its open session stops"
        : `its ${sessions} open sessions stop`,
    );
  let text = `${parts.join(", and ")}.`;
  if (!worktree.merged && worktree.ahead)
    text += ` The branch has ${worktree.ahead} ${worktree.ahead === 1 ? "commit" : "commits"} that ${worktree.ahead === 1 ? "is" : "are"} not in ${base}.`;
  return text;
}
