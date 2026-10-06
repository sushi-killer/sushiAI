import type { WorktreeClaim } from "../extensions/modules.ts";

/** Stub: the worktrees the orchestrator owns. Lane L4b fills it in. */
export function useOrchestratorWorktreeClaims(
  enabled: boolean,
): WorktreeClaim[] {
  void enabled;
  return [];
}
