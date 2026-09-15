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
 * member by its branch alone once it's this Mac's checkout - fine inside the
 * sidebar row, which already carries its own "Local" context, but ambiguous
 * standing next to a differently-labelled sibling worktree in a plain list -
 * so here a worktree group's Local member gets its host prefixed too. */
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
  return group.members.map((member) => {
    const label = memberLabel(group, member, connectionProfiles);
    const named =
      group.worktrees && member.hostKey === LOCAL_GROUP
        ? `${groupLabel(member.hostKey, connectionProfiles)} · ${label}`
        : label;
    return { workspaceId: member.workspace.id, label: named };
  });
}
