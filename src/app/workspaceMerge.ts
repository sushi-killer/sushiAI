import type { ConnectionProfile, Workspace } from "../types";
import type { ProjectGit } from "./useProjectGit";

/** Groups the flat workspace list by which Herdr this Mac or an SSH host owns
 * it. Shared by Sidebar (which draws the grouping) and App (which needs to
 * know a pane's own host without drawing anything). */
export const LOCAL_GROUP = "local";
export function groupKey(connection?: string) {
  return connection?.startsWith("ssh:") ? connection : LOCAL_GROUP;
}
export function groupLabel(key: string, profiles: ConnectionProfile[]) {
  if (key === LOCAL_GROUP) return "Local";
  return (
    profiles.find((p) => `ssh:${p.id}` === key)?.name ||
    key.replace(/^ssh:/, "")
  );
}
/** Every connected host is polled independently (see `useHerdr`), so a
 * group's status is always its own endpoint's real, current poll result -
 * never borrowed from whichever connection happens to be the default one. */
export function groupStatus(
  key: string,
  localSocket: string,
  statusByEndpoint: Record<string, string>,
) {
  const endpoint = key === LOCAL_GROUP ? localSocket : key;
  return statusByEndpoint[endpoint] || "connecting";
}
/** A profile hidden from the sidebar still keeps its tunnel - this only
 * decides whether its workspaces are ever offered here. */
export function isHidden(
  connection: string | undefined,
  profiles: ConnectionProfile[],
) {
  if (!connection?.startsWith("ssh:")) return false;
  return Boolean(profiles.find((p) => `ssh:${p.id}` === connection)?.hidden);
}
/** Which projects (identity from the project store, else the remote) run on
 * both this Mac and a remote host - every workspace sharing that identity gets a small "mixed" marker, since the
 * point is "this project spans machines," not which specific copy you're
 * looking at. This is coarser than merge identity: it only compares the
 * identity, not the subdir each checkout runs from or the same-host clone
 * ambiguity that keeps computeMergeGroups from joining two checkouts - so it
 * still fires for a workspace that is flagged "mixed" but does not qualify to
 * merge - that row keeps today's passive marker. */
export function mixedRemotes(
  visible: Workspace[],
  projectGit: Record<string, ProjectGit>,
) {
  const mix = new Map<string, { local: boolean; remote: boolean }>();
  for (const w of visible) {
    const info = projectGit[w.id];
    const identity = info && (info.projectId || info.remote);
    if (!identity) continue;
    const entry = mix.get(identity) || { local: false, remote: false };
    if (groupKey(w.connection) === LOCAL_GROUP) entry.local = true;
    else entry.remote = true;
    mix.set(identity, entry);
  }
  const mixed = new Set<string>();
  for (const [remote, entry] of mix)
    if (entry.local && entry.remote) mixed.add(remote);
  return mixed;
}

function basenameOf(path: string): string {
  return (path || "").replace(/\/+$/, "").split("/").pop() ?? "";
}

export type MergedMember = {
  workspace: Workspace;
  hostKey: string;
  git: ProjectGit;
};
/** One merged row: every member is a distinct checkout - another host, or
 * another worktree of the same repository on one host (see
 * `computeMergeGroups`). `members` is already ordered by AC11 - this Mac
 * first, then remote hosts by label A->Z, and within a host the main
 * checkout before its worktrees by branch. `id` is the merge identity,
 * stable across re-renders and across toggling grouping, so a UI can key its
 * own expansion state on it. `worktrees` is true when some host contributes
 * more than one checkout, which is when a member's host alone no longer
 * names it. */
export type MergeGroup = {
  id: string;
  members: MergedMember[];
  worktrees: boolean;
};

function sortMembers(
  members: MergedMember[],
  profiles: ConnectionProfile[],
): MergedMember[] {
  const isMain = (m: MergedMember) =>
    `${m.git.checkout}/.git` === m.git.commonDir;
  return [...members].sort((a, b) => {
    if (a.hostKey !== b.hostKey) {
      if (a.hostKey === LOCAL_GROUP) return -1;
      if (b.hostKey === LOCAL_GROUP) return 1;
      return groupLabel(a.hostKey, profiles).localeCompare(
        groupLabel(b.hostKey, profiles),
      );
    }
    if (isMain(a) !== isMain(b)) return isMain(a) ? -1 : 1;
    return a.git.branch.localeCompare(b.git.branch);
  });
}

/** A host's members that are each alone in their checkout - one checkout
 * opened twice contributes neither (AC8), since picking one would silently
 * hide a real duplicate. */
function distinctCheckouts(members: MergedMember[]): MergedMember[] {
  const byCheckout = new Map<string, MergedMember[]>();
  for (const m of members)
    byCheckout.set(m.git.checkout, [
      ...(byCheckout.get(m.git.checkout) ?? []),
      m,
    ]);
  return [...byCheckout.values()]
    .filter((same) => same.length === 1)
    .map(([m]) => m);
}

/** Merge identity (D4): the same folder inside one repository - worktrees
 * sharing a common dir on one host, or across hosts the same project id from
 * the project store (else a non-empty normalized remote), whatever each
 * checkout's folder is called.
 * Separate clones never share a common dir, so they never merge with each
 * other, and a host holding two clones cannot say which one a remote host's
 * checkout matches, so neither joins the cross-host row (product-brief edge
 * case 3) - each clone's own worktrees still merge. Returned as a lookup by
 * workspace id, because each sidebar list or active workspace needs to know
 * which group, if any, a workspace belongs to. */
export function computeMergeGroups(
  visible: Workspace[],
  projectGit: Record<string, ProjectGit>,
  connectionProfiles: ConnectionProfile[],
): Map<string, MergeGroup> {
  const buckets = new Map<string, Map<string, MergedMember[]>>();
  for (const w of visible) {
    const git = projectGit[w.id];
    if (!git?.commonDir || !git.checkout) continue;
    const hostKey = groupKey(w.connection);
    const repository =
      git.projectId || git.remote || `${hostKey}::${git.commonDir}`;
    const identity = `${repository}::${git.subdir}`;
    const byHost = buckets.get(identity) ?? new Map<string, MergedMember[]>();
    byHost.set(hostKey, [
      ...(byHost.get(hostKey) ?? []),
      { workspace: w, hostKey, git },
    ]);
    buckets.set(identity, byHost);
  }
  const byWorkspaceId = new Map<string, MergeGroup>();
  const addGroup = (id: string, members: MergedMember[]) => {
    if (members.length < 2) return;
    const group: MergeGroup = {
      id,
      members: sortMembers(members, connectionProfiles),
      worktrees: new Set(members.map((m) => m.hostKey)).size < members.length,
    };
    for (const member of group.members)
      byWorkspaceId.set(member.workspace.id, group);
  };
  for (const [identity, byHost] of buckets) {
    const shared: MergedMember[] = [];
    for (const [hostKey, onHost] of byHost) {
      const byRepository = new Map<string, MergedMember[]>();
      for (const m of onHost)
        byRepository.set(m.git.commonDir, [
          ...(byRepository.get(m.git.commonDir) ?? []),
          m,
        ]);
      if (byRepository.size === 1) {
        shared.push(...distinctCheckouts(onHost));
        continue;
      }
      for (const [commonDir, clone] of byRepository)
        addGroup(
          `${hostKey}::${commonDir}::${identity}`,
          distinctCheckouts(clone),
        );
    }
    addGroup(identity, shared);
  }
  return byWorkspaceId;
}

/** Worktree groups for the sidebar's host sections. Scoping each host before
 * computing groups keeps a project on another host in its own section. */
export function computeHostMergeGroups(
  visible: Workspace[],
  projectGit: Record<string, ProjectGit>,
  connectionProfiles: ConnectionProfile[],
): Map<string, MergeGroup> {
  const byHost = new Map<string, Workspace[]>();
  for (const workspace of visible) {
    const key = groupKey(workspace.connection);
    byHost.set(key, [...(byHost.get(key) ?? []), workspace]);
  }

  const groups = new Map<string, MergeGroup>();
  for (const members of byHost.values()) {
    for (const [workspaceId, group] of computeMergeGroups(
      members,
      projectGit,
      connectionProfiles,
    ))
      groups.set(workspaceId, group);
  }
  return groups;
}

/** The slice of an orchd task the sidebar needs to name its worktree. */
export type WorktreeTask = {
  title: string;
  branch: string;
  worktree: string;
  repo: string;
  updatedAt: number;
  status: string;
};

const trimSlashes = (path: string) => (path || "").replace(/\/+$/, "");

/** The orchd task a local member's checkout belongs to: by worktree path, or
 * by branch inside the same repository. The newest match wins. */
export function memberTask(
  member: MergedMember,
  tasks: WorktreeTask[],
): WorktreeTask | undefined {
  if (member.hostKey !== LOCAL_GROUP) return undefined;
  const { checkout, branch, commonDir } = member.git;
  let best: WorktreeTask | undefined;
  for (const task of tasks) {
    const match =
      (Boolean(task.worktree) &&
        trimSlashes(task.worktree) === trimSlashes(checkout)) ||
      (Boolean(task.branch) &&
        task.branch === branch &&
        `${trimSlashes(task.repo)}/.git` === commonDir);
    if (match && (!best || task.updatedAt > best.updatedAt)) best = task;
  }
  return best;
}

/** What names one member inside its row: the host, as before, until a host
 * contributes several worktrees - then the orchd task's title when the
 * worktree belongs to one, else the branch, prefixed by the host only off
 * this Mac (a detached checkout reports its short commit). */
export function memberLabel(
  group: MergeGroup,
  member: MergedMember,
  profiles: ConnectionProfile[],
  tasks: WorktreeTask[] = [],
): string {
  const host = groupLabel(member.hostKey, profiles);
  if (!group.worktrees) return host;
  const title = memberTask(member, tasks)?.title;
  if (title) return title;
  const branch = member.git.branch || basenameOf(member.git.checkout);
  return member.hostKey === LOCAL_GROUP ? branch : `${host} · ${branch}`;
}

/** The label, plus the branch when the label is a task title. */
export function memberTooltip(
  group: MergeGroup,
  member: MergedMember,
  profiles: ConnectionProfile[],
  tasks: WorktreeTask[] = [],
): string {
  const label = memberLabel(group, member, profiles, tasks);
  const titled = group.worktrees && memberTask(member, tasks);
  return titled && member.git.branch
    ? `${label} (${member.git.branch})`
    : label;
}

/** D2: the merged row's single status dot follows the active member, else
 * the Local member, else the first member in AC11 order. Never a third,
 * "mixed" appearance. */
export function mergedRowStatusKey(
  group: MergeGroup,
  activeWorkspaceId: string,
): string {
  const active = group.members.find(
    (m) => m.workspace.id === activeWorkspaceId,
  );
  if (active) return active.hostKey;
  const local = group.members.find((m) => m.hostKey === LOCAL_GROUP);
  if (local) return local.hostKey;
  return group.members[0].hostKey;
}

/** Section D of the visual contract: collapse every marker to one `<n>
 * hosts` (or `<n> checkouts`) chip at 3+ members, or when the members'
 * labels total more than 12 characters. A bare Local never counts - it is
 * a fixed, user-can't-change string - but a branch always does. */
export function shouldCollapseHostMarkers(
  group: MergeGroup,
  profiles: ConnectionProfile[],
  tasks: WorktreeTask[] = [],
): boolean {
  if (group.members.length >= 3) return true;
  const charCount = group.members
    .filter((m) => group.worktrees || m.hostKey !== LOCAL_GROUP)
    .reduce(
      (total, m) => total + memberLabel(group, m, profiles, tasks).length,
      0,
    );
  return charCount > 12;
}

function stateWord(status: string): "Connected" | "Connecting" | "Offline" {
  return status === "connected"
    ? "Connected"
    : status === "connecting"
      ? "Connecting"
      : "Offline";
}

/** UX6: the marker group's accessible name/title carries every member's
 * state word, because AC13 requires all of them readable without expanding
 * the row - AC31's plain "Runs on <A> and <B>." has no room for that. No
 * Oxford comma, matching the app's plain register. */
export function mergedMarkerAccessibleName(
  group: MergeGroup,
  profiles: ConnectionProfile[],
  localSocket: string,
  statusByEndpoint: Record<string, string>,
  tasks: WorktreeTask[] = [],
): string {
  const parts = group.members.map((m) => {
    const label = memberLabel(group, m, profiles, tasks);
    const status = groupStatus(m.hostKey, localSocket, statusByEndpoint);
    return `${label} (${stateWord(status)})`;
  });
  const joined =
    parts.length <= 1
      ? parts.join("")
      : parts.length === 2
        ? `${parts[0]} and ${parts[1]}`
        : `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
  return `${group.worktrees ? "Checked out as" : "Runs on"} ${joined}.`;
}

/** The merge group the active workspace belongs to, or `undefined` when it
 * stands alone - grouped mode always answers `undefined`, because merging is
 * flat-mode only. Anything drawn per merged project (the sidebar row, pane
 * provenance, a canvas spanning its members) starts from this. */
export function activeMergeGroup(
  workspaces: Workspace[],
  active: Workspace,
  projectGit: Record<string, ProjectGit>,
  connectionProfiles: ConnectionProfile[],
  workspaceGrouping: "grouped" | "flat",
): MergeGroup | undefined {
  if (workspaceGrouping !== "flat") return undefined;
  const visible = workspaces.filter(
    (w) => !isHidden(w.connection, connectionProfiles),
  );
  return computeMergeGroups(visible, projectGit, connectionProfiles).get(
    active.id,
  );
}
