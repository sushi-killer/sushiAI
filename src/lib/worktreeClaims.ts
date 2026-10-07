import type { WorktreeClaim } from "../extensions/modules.ts";

const trimSlashes = (path: string) => (path || "").replace(/\/+$/, "");

/** The claim a checkout belongs to: on the same host, by worktree path, or by
 * branch inside the same repository (named by its root or by its git common
 * dir). An active claim wins over an inactive one on the same checkout, then
 * the module's own order. The one matcher for the sidebar and the Worktrees
 * table. */
export function claimFor(
  claims: WorktreeClaim[],
  host: string,
  at: { path: string; branch?: string; repo?: string; commonDir?: string },
): WorktreeClaim | undefined {
  const mine = claims.filter(
    (claim) =>
      claim.host === host &&
      ((!!claim.path && trimSlashes(claim.path) === trimSlashes(at.path)) ||
        (!!claim.branch &&
          claim.branch === at.branch &&
          ((at.repo !== undefined &&
            trimSlashes(claim.repo) === trimSlashes(at.repo)) ||
            (at.commonDir !== undefined &&
              `${trimSlashes(claim.repo)}/.git` === at.commonDir)))),
  );
  return mine.find((claim) => claim.active) ?? mine[0];
}
