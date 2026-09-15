import type { ConnectionProfile, Workspace } from "../types";
import type { ClosedProject } from "../workspaceState.ts";
import { codePanels } from "../workspaceState.ts";
import type { ProjectGit } from "./useProjectGit.ts";
import {
  computeMergeGroups,
  isHidden,
  memberLabel,
} from "./workspaceMerge.ts";

/** A closed project's identity is its host endpoint + cwd - the same pair
 * that decides whether reopening it would just recreate an already-open
 * workspace. Used both as the remembered entry's `id` and as the pseudo
 * workspace id merging keys off (`closed:<endpoint>:<cwd>`), so the two never
 * drift apart. */
export function closedProjectId(
  endpoint: string | undefined,
  cwd: string,
): string {
  return `closed:${endpoint || "local"}:${cwd}`;
}

function isClosedId(id: string): boolean {
  return id.startsWith("closed:");
}

/** Remembers a closed project, deduped by endpoint+cwd (D1): closing the
 * same project again refreshes its entry instead of piling up a duplicate.
 * Most-recently-closed first. */
export function rememberProject(
  list: ClosedProject[],
  entry: ClosedProject,
): ClosedProject[] {
  return [entry, ...list.filter((p) => p.id !== entry.id)];
}

/** Drops a remembered project - used both for the "Remove from projects"
 * control and once a reopen succeeds. */
export function forgetProject(
  list: ClosedProject[],
  id: string,
): ClosedProject[] {
  return list.filter((p) => p.id !== id);
}

function pseudoWorkspace(project: ClosedProject): Workspace {
  return {
    id: project.id,
    name: project.name,
    cwd: project.cwd,
    connection: project.endpoint,
    panels: [],
    layout: null,
  };
}

export type DashboardMember = {
  id: string;
  label: string;
  closed: boolean;
};

export type DashboardEntry = {
  id: string;
  /** The one member a card's own click acts on: `switchWorkspace` when open,
   * `reopenProject`/`forgetProject` (by id, against `closedProjects`) when
   * closed. Distinct from `id` because a merged card's `id` is the merge
   * group's identity, which never matches any single member. */
  primaryId: string;
  name: string;
  cwd: string;
  panelCount: number;
  /** True only when every member is a closed project - a group with at least
   * one open member is drawn like any other open project, with its closed
   * member(s) marked individually. */
  closed: boolean;
  members: DashboardMember[];
};

/** The Dashboard's project list (B4): every open workspace and every
 * remembered closed project, merged exactly like the sidebar's flat mode -
 * always, independent of the sidebar's own grouping toggle, because the
 * Dashboard has no grouped view of its own. A closed project already
 * reopened as an open workspace (same endpoint+cwd) is dropped here rather
 * than shown twice; the caller still owns forgetting the stale entry. */
export function dashboardEntries(
  workspaces: Workspace[],
  closedProjects: ClosedProject[],
  projectGit: Record<string, ProjectGit>,
  connectionProfiles: ConnectionProfile[],
): DashboardEntry[] {
  const openKeys = new Set(
    workspaces.map((w) => `${w.connection || ""}::${w.cwd}`),
  );
  const closedVisible = closedProjects.filter(
    (p) =>
      !isHidden(p.endpoint, connectionProfiles) &&
      !openKeys.has(`${p.endpoint || ""}::${p.cwd}`),
  );
  const visibleWorkspaces = workspaces.filter(
    (w) => !isHidden(w.connection, connectionProfiles),
  );
  const combined = [
    ...visibleWorkspaces,
    ...closedVisible.map(pseudoWorkspace),
  ];
  const combinedGit: Record<string, ProjectGit> = { ...projectGit };
  for (const project of closedVisible) combinedGit[project.id] = project.git;
  const groups = computeMergeGroups(combined, combinedGit, connectionProfiles);
  const seenGroups = new Set<string>();
  const entries: DashboardEntry[] = [];
  for (const w of combined) {
    const group = groups.get(w.id);
    if (group) {
      if (seenGroups.has(group.id)) continue;
      seenGroups.add(group.id);
      const open = group.members.find((m) => !isClosedId(m.workspace.id));
      const representative = open || group.members[0];
      entries.push({
        id: group.id,
        primaryId: representative.workspace.id,
        name: representative.workspace.name,
        cwd: representative.workspace.cwd,
        panelCount: codePanels(representative.workspace).length,
        closed: !open,
        members: group.members.map((m) => ({
          id: m.workspace.id,
          label: memberLabel(group, m, connectionProfiles),
          closed: isClosedId(m.workspace.id),
        })),
      });
    } else {
      entries.push({
        id: w.id,
        primaryId: w.id,
        name: w.name,
        cwd: w.cwd,
        panelCount: codePanels(w).length,
        closed: isClosedId(w.id),
        members: [{ id: w.id, label: w.name, closed: isClosedId(w.id) }],
      });
    }
  }
  return entries;
}
