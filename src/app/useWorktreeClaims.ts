import { moduleUis } from "../extensions/modules.ts";
import type { WorktreeClaim } from "../extensions/modules.ts";

/** Every module's worktree claims, in module order. */
export function useWorktreeClaims(): WorktreeClaim[] {
  // moduleUis is a constant list, so the hooks run in a fixed order.
  const lists = moduleUis.map((module) => module.useWorktreeClaims());
  return lists.flat();
}
