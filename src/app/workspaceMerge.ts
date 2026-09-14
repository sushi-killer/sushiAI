import type { ConnectionProfile, Workspace } from "../types";

/** Groups the flat workspace list by which Herdr this Mac or an SSH host owns
 * it. Shared by Sidebar (which draws the grouping) and App (which needs to
 * know a pane's own host without drawing anything). */
export const LOCAL_GROUP = "local";
export function groupKey(connection?: string) {
  return connection?.startsWith("ssh:") ? connection : LOCAL_GROUP;
}
export function groupLabel(key: string, profiles: ConnectionProfile[]) {
  if (key === LOCAL_GROUP) return "This Mac";
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
 * looking at. This is coarser than merge identity (no basename check), so it
 * still fires for a workspace that is flagged "mixed" but does not qualify to
 * merge (different basename) - that row keeps today's passive marker. */
export function mixedRemotes(
  visible: Workspace[],
  projectRemotes: Record<string, string>,
) {
  const mix = new Map<string, { local: boolean; remote: boolean }>();
  for (const w of visible) {
    const remote = projectRemotes[w.id];
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

function basenameOf(cwd: string): string {
  return (cwd || "").replace(/\/+$/, "").split("/").pop()?.toLowerCase() ?? "";
}

export type MergedMember = { workspace: Workspace; hostKey: string };
/** One merged row: every member is on a different host and stands alone on
 * that host (see `computeMergeGroups`). `members` is already ordered by
 * AC11 - this Mac first, then remote hosts by label A->Z. `id` is the merge
 * identity (`remote::basename`), stable across re-renders and across
 * toggling grouping, so a UI can key its own expansion state on it. */
export type MergeGroup = { id: string; members: MergedMember[] };

function sortMembers(
  members: MergedMember[],
  profiles: ConnectionProfile[],
): MergedMember[] {
  return [...members].sort((a, b) => {
    if (a.hostKey === LOCAL_GROUP) return -1;
    if (b.hostKey === LOCAL_GROUP) return 1;
    return groupLabel(a.hostKey, profiles).localeCompare(
      groupLabel(b.hostKey, profiles),
    );
  });
}

/** Merge identity (D4): a non-empty normalized git remote, an equal `cwd`
 * basename (case-insensitive), and members on different hosts. Fails toward
 * not merging - two workspaces that already collide on one host under this
 * same identity contribute neither to the group, since picking one of them
 * to represent that host would silently hide a real duplicate (product-brief
 * edge case 3, AC8). Returned as a lookup by workspace id, because the
 * caller (Sidebar's flat list, or a single active workspace) always starts
 * from one workspace and needs to know which group, if any, it belongs to. */
export function computeMergeGroups(
  visible: Workspace[],
  projectRemotes: Record<string, string>,
  connectionProfiles: ConnectionProfile[],
): Map<string, MergeGroup> {
  const buckets = new Map<string, Map<string, Workspace[]>>();
  for (const w of visible) {
    const remote = projectRemotes[w.id];
    if (!remote) continue;
    const basename = basenameOf(w.cwd);
    if (!basename) continue;
    const identity = `${remote}::${basename}`;
    const hostKey = groupKey(w.connection);
    const byHost = buckets.get(identity) ?? new Map<string, Workspace[]>();
    byHost.set(hostKey, [...(byHost.get(hostKey) ?? []), w]);
    buckets.set(identity, byHost);
  }
  const byWorkspaceId = new Map<string, MergeGroup>();
  for (const [identity, byHost] of buckets) {
    const members: MergedMember[] = [];
    for (const [hostKey, onHost] of byHost) {
      if (onHost.length !== 1) continue; // same host twice (AC8): neither joins the merge
      members.push({ workspace: onHost[0], hostKey });
    }
    if (members.length < 2) continue; // needs at least two distinct hosts
    const group: MergeGroup = {
      id: identity,
      members: sortMembers(members, connectionProfiles),
    };
    for (const member of group.members)
      byWorkspaceId.set(member.workspace.id, group);
  }
  return byWorkspaceId;
}

/** D2: the merged row's single status dot follows the active member, else
 * This Mac's member, else the first member in AC11 order. Never a third,
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
 * hosts` chip at 3+ members, or when the remote members' labels (This Mac
 * never counts - it is a fixed, user-can't-change string) total more than
 * 12 characters. */
export function shouldCollapseHostMarkers(
  group: MergeGroup,
  profiles: ConnectionProfile[],
): boolean {
  if (group.members.length >= 3) return true;
  const remoteCharCount = group.members
    .filter((m) => m.hostKey !== LOCAL_GROUP)
    .reduce((total, m) => total + groupLabel(m.hostKey, profiles).length, 0);
  return remoteCharCount > 12;
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
    const label = groupLabel(m.hostKey, profiles);
    const status = groupStatus(m.hostKey, localSocket, statusByEndpoint);
    return `${label} (${stateWord(status)})`;
  });
  const joined =
    parts.length <= 1
      ? parts.join("")
      : parts.length === 2
        ? `${parts[0]} and ${parts[1]}`
        : `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
  return `Runs on ${joined}.`;
}

/** Pane provenance (AC23-AC26, D5): the host label a merged workspace's own
 * panes should carry, or `undefined` when the active workspace isn't a
 * member of any merged row - grouped mode always answers `undefined`
 * (merging is flat-mode only), which is what makes AC25 true by
 * construction rather than by a second check at the call site. */
export function activeMergedHostLabel(
  workspaces: Workspace[],
  active: Workspace,
  projectRemotes: Record<string, string>,
  connectionProfiles: ConnectionProfile[],
  workspaceGrouping: "grouped" | "flat",
): string | undefined {
  if (workspaceGrouping !== "flat") return undefined;
  const visible = workspaces.filter(
    (w) => !isHidden(w.connection, connectionProfiles),
  );
  const group = computeMergeGroups(
    visible,
    projectRemotes,
    connectionProfiles,
  ).get(active.id);
  if (!group) return undefined;
  const member = group.members.find((m) => m.workspace.id === active.id);
  return member ? groupLabel(member.hostKey, connectionProfiles) : undefined;
}
