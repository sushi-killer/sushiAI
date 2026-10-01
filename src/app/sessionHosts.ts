import type { ConnectionProfile, Project, Workspace } from "../types";
import { normalizeRemote, type ProjectGit } from "./useProjectGit.ts";
import {
  activeMergeGroup,
  groupLabel,
  type MergedMember,
} from "./workspaceMerge.ts";

/** One representative checkout per host in the active workspace's merge group. */
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
  /** Opens a workspace on a prepared host and starts `starter` in it. */
  startOnHost(
    name: string,
    cwd: string,
    endpoint: string,
    starter: string,
  ): Promise<boolean>;
};

/** The projects the "+" picker can switch to: those that have a workspace
 * open, each with the workspace the picker then targets. */
export function projectChoices(
  projects: Project[],
  workspaces: Workspace[],
  projectGit: Record<string, ProjectGit>,
): { project: Project; workspace: Workspace }[] {
  const choices: { project: Project; workspace: Workspace }[] = [];
  for (const project of projects) {
    const key = normalizeRemote(project.git.url);
    // By its remote, or, for a folder with none, by the folder it was
    // attached to.
    const workspace = workspaces.find((item) => {
      const remote = projectGit[item.id]?.remote;
      if (key && remote && normalizeRemote(remote) === key) return true;
      const host = item.connection?.startsWith("ssh:")
        ? item.connection
        : "local";
      return (project.folders ?? []).some(
        (folder) => folder.endpoint === host && folder.cwd === item.cwd,
      );
    });
    if (workspace) choices.push({ project, workspace });
  }
  return choices;
}

/** The workspace a launch lands in: the picked host of a merge group, or, in
 * a project switched to from the title, that project's own workspace. Only
 * the plain case (no group, no switch) leaves it to the active workspace. */
export function launchTarget(
  hostCount: number,
  hostId: string,
  baseId: string,
  openedId: string,
): string | undefined {
  return hostCount > 0 || baseId !== openedId ? hostId : undefined;
}

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
  const hosts = new Map<string, MergedMember>();
  for (const member of group.members)
    if (!hosts.has(member.hostKey) || member.workspace.id === active.id)
      hosts.set(member.hostKey, member);
  return [...hosts.values()].map((member) => ({
    workspaceId: member.workspace.id,
    label: groupLabel(member.hostKey, connectionProfiles),
  }));
}
