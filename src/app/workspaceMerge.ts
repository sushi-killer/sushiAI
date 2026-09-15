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
/** Which projects (by git remote) run on both this Mac and a remote host -
 * every workspace sharing that remote gets a small "mixed" marker, since the
 * point is "this project spans machines," not which specific copy you're
 * looking at. This is coarser than merge identity (no repository-name check),
 * so it still fires for a workspace that is flagged "mixed" but does not
 * qualify to merge - that row keeps today's passive marker. */
export function mixedRemotes(
  visible: Workspace[],
  projectGit: Record<string, ProjectGit>,
) {
  const mix = new Map<string, { local: boolean; remote: boolean }>();
  for (const w of visible) {
    const remote = projectGit[w.id]?.remote;
    if (!remote) continue;
    const entry = mix.get(remote) || { local: false, remote: false };
    if (groupKey(w.connection) === LOCAL_GROUP) entry.local = true;
    else entry.remote = true;
    mix.set(remote, entry);
  }
  const mixed = new Set<string>();
  for (const [remote, entry] of mix)
    if (entry.local && entry.remote) mixed.add(remote);
  return mixed;
}

function basenameOf(path: string): string {
  return (path || "").replace(/\/+$/, "").split("/").pop() ?? "";
}

/** The main repository's folder name, read from the shared git dir rather
 * than the cwd, so a worktree named `repo-feature` still reads as `repo`. */
function repositoryName(commonDir: string): string {
  const name = basenameOf(commonDir);
  const folder =
    name === ".git" ? basenameOf(commonDir.replace(/\/+[^/]*\/*$/, "")) : name;
  return folder.replace(/\.git$/i, "").toLowerCase();
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
 * sharing a git common dir on one host, or across hosts a non-empty
 * normalized git remote plus an equal repository name (case-insensitive).
 * Separate clones never share a common dir, so they never merge with each
 * other, and a host holding two clones cannot say which one a remote host's
 * checkout matches, so neither joins the cross-host row (product-brief edge
 * case 3) - each clone's own worktrees still merge. Returned as a lookup by
 * workspace id, because the caller (Sidebar's flat list, or a single active
 * workspace) always starts from one workspace and needs to know which group,
 * if any, it belongs to. */
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
    const repository = git.remote
      ? `${git.remote}::${repositoryName(git.commonDir)}`
      : `${hostKey}::${git.commonDir}`;
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

/** What names one member inside its row: the host, as before, until a host
 * contributes several worktrees - then the branch, prefixed by the host only
 * off this Mac (a detached checkout reports its short commit). */
export function memberLabel(
  group: MergeGroup,
  member: MergedMember,
  profiles: ConnectionProfile[],
): string {
  const host = groupLabel(member.hostKey, profiles);
  if (!group.worktrees) return host;
  const branch = member.git.branch || basenameOf(member.git.checkout);
  return member.hostKey === LOCAL_GROUP ? branch : `${host} · ${branch}`;
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
): boolean {
  if (group.members.length >= 3) return true;
  const charCount = group.members
    .filter((m) => group.worktrees || m.hostKey !== LOCAL_GROUP)
    .reduce((total, m) => total + memberLabel(group, m, profiles).length, 0);
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
): string {
  const parts = group.members.map((m) => {
    const label = memberLabel(group, m, profiles);
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

/** Pane provenance (AC23-AC26, D5): the member label a merged workspace's
 * own panes should carry, or `undefined` when the active workspace isn't a
 * member of any merged row - grouped mode always answers `undefined`
 * (merging is flat-mode only), which is what makes AC25 true by
 * construction rather than by a second check at the call site. */
export function activeMergedHostLabel(
  workspaces: Workspace[],
  active: Workspace,
  projectGit: Record<string, ProjectGit>,
  connectionProfiles: ConnectionProfile[],
  workspaceGrouping: "grouped" | "flat",
): string | undefined {
  if (workspaceGrouping !== "flat") return undefined;
  const visible = workspaces.filter(
    (w) => !isHidden(w.connection, connectionProfiles),
  );
  const group = computeMergeGroups(visible, projectGit, connectionProfiles).get(
    active.id,
  );
  if (!group) return undefined;
  const member = group.members.find((m) => m.workspace.id === active.id);
  return member ? memberLabel(group, member, connectionProfiles) : undefined;
}
