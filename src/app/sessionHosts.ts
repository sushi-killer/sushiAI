import type { ConnectionProfile, Workspace } from "../types";
import type { ProjectGit } from "./useProjectGit.ts";
import {
  activeMergeGroup,
  groupLabel,
  LOCAL_GROUP,
  memberLabel,
} from "./workspaceMerge.ts";

/** One host the new-session picker can launch into: a merge-group member's
 * own workspace, on its own connection. Only the active workspace's merge
 * group (flat sidebar mode, D1) ever produces more than one - a workspace
 * that stands alone keeps the picker's plain, single-host behaviour (D3). */
export type SessionHostOption = {
  workspaceId: string;
  label: string;
};

/** Everything `sessionHostOptions` needs beyond the active workspace - one
 * object so App.tsx (a props-wiring-only file) hands it down as a single
 * prop instead of four. */
export type SessionHostContext = {
  workspaces: Workspace[];
  projectGit: Record<string, ProjectGit>;
  connectionProfiles: ConnectionProfile[];
  workspaceGrouping: "grouped" | "flat";
};

/** D1: one option per merge-group member. `memberLabel` names a worktree
 * member on this Mac by its branch alone. That is enough while every member
 * is on one machine, but next to `Lab · feature-x` a bare `main` hides where
 * it runs - so only a group spanning hosts prefixes the Local member. */
export function sessionHostOptions(
  active: Workspace,
  {
    workspaces,
    projectGit,
    connectionProfiles,
    workspaceGrouping,
  }: SessionHostContext,
): SessionHostOption[] {
  const group = activeMergeGroup(
    workspaces,
    active,
    projectGit,
    connectionProfiles,
    workspaceGrouping,
  );
  if (!group) return [];
  const manyHosts = new Set(group.members.map((m) => m.hostKey)).size > 1;
  return group.members.map((member) => {
    const label = memberLabel(group, member, connectionProfiles);
    const named =
      group.worktrees && manyHosts && member.hostKey === LOCAL_GROUP
        ? `${groupLabel(member.hostKey, connectionProfiles)} · ${label}`
        : label;
    return { workspaceId: member.workspace.id, label: named };
  });
}
