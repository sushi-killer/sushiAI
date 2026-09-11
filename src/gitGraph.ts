/**
 * Pure layout logic for the branch/commit graph shown in the Files & Git panel.
 * Turns a flat `git log --topo-order` commit list into lanes + edges a lightweight
 * SVG renderer can draw, the same way `git log --graph` lays out ASCII lanes.
 */
export type GitLogCommit = {
  hash: string;
  parents: string[];
  author: string;
  /** Unix seconds, as reported by `%ct`. */
  date: number;
  subject: string;
  /** Raw decoration tokens from `%D`, e.g. "HEAD -> main", "origin/main", "tag: v1.0". */
  refs: string[];
};

export type RefBadge = {
  label: string;
  kind: "head" | "branch" | "remote" | "tag";
};

/** One lane→lane segment spanning from the bottom of `row` to the top of `row + 1`. */
export type GraphEdge = {
  row: number;
  fromLane: number;
  toLane: number;
};

export type GraphNode = {
  hash: string;
  row: number;
  lane: number;
  /** Whether this row's commit already had a line arriving from above. */
  hasIncoming: boolean;
};

export type GitGraphLayout = {
  nodes: GraphNode[];
  edges: GraphEdge[];
  laneCount: number;
};

/** Assigns a lane per commit and the edges connecting consecutive rows. */
export function computeGitGraph(commits: GitLogCommit[]): GitGraphLayout {
  let tracks: (string | null)[] = [];
  const nodes: GraphNode[] = [];
  const edges: GraphEdge[] = [];
  let laneCount = 0;
  const byHash = new Map(commits.map((commit) => [commit.hash, commit]));

  const findTrackedAncestor = (
    parents: string[],
    currentTracks: (string | null)[],
  ): number => {
    const queue = [...parents];
    const visited = new Set<string>();
    while (queue.length) {
      const hash = queue.shift()!;
      const lane = currentTracks.indexOf(hash);
      if (lane !== -1) return lane;
      if (visited.has(hash)) continue;
      visited.add(hash);
      queue.push(...(byHash.get(hash)?.parents || []));
    }
    return -1;
  };

  commits.forEach((commit, row) => {
    let lane = tracks.indexOf(commit.hash);
    const hasIncoming = lane !== -1;
    const forkFromLane = hasIncoming
      ? -1
      : findTrackedAncestor(commit.parents, tracks);
    if (lane === -1) {
      lane = tracks.indexOf(null);
      if (lane === -1) lane = tracks.length;
      tracks[lane] = commit.hash;
    }
    laneCount = Math.max(laneCount, lane + 1);
    nodes.push({ hash: commit.hash, row, lane, hasIncoming });

    // A branch commit may appear before its parent lane is consumed. Draw the
    // fork on the preceding row so the new node is joined to its real parent,
    // instead of appearing as a dot with a blank gap above it.
    if (forkFromLane !== -1 && row > 0)
      edges.push({ row: row - 1, fromLane: forkFromLane, toLane: lane });

    // Build the next row first. This lets us compact empty lanes immediately
    // after a merge, so the graph never leaves a dead vertical gutter behind.
    const nextTracks = tracks.slice();
    nextTracks[lane] = null;
    const parentLanes: number[] = [];
    for (const parent of commit.parents) {
      let target = nextTracks.indexOf(parent);
      if (target === -1) {
        target = nextTracks.indexOf(null);
        if (target === -1) target = nextTracks.length;
        nextTracks[target] = parent;
      }
      parentLanes.push(target);
    }

    laneCount = Math.max(laneCount, nextTracks.length);
    const nextLaneByOldLane = new Map<number, number>();
    const compacted: (string | null)[] = [];
    nextTracks.forEach((track, oldLane) => {
      if (track === null || compacted.includes(track)) return;
      nextLaneByOldLane.set(oldLane, compacted.length);
      compacted.push(track);
    });

    // Lanes not touched by this commit continue straight into the compacted
    // next row. The current commit fans out to each unique parent lane.
    tracks.forEach((track, oldLane) => {
      if (oldLane === lane || track === null) return;
      const nextOldLane = nextTracks.indexOf(track);
      const nextLane = nextLaneByOldLane.get(nextOldLane);
      if (nextLane !== undefined)
        edges.push({ row, fromLane: oldLane, toLane: nextLane });
    });
    for (const parentLane of parentLanes) {
      const nextLane = nextLaneByOldLane.get(parentLane);
      if (nextLane !== undefined)
        edges.push({ row, fromLane: lane, toLane: nextLane });
    }

    tracks = compacted;
  });

  return { nodes, edges, laneCount };
}

const PALETTE = [
  "#8bb38a",
  "#82a0c8",
  "#d1b883",
  "#c090c9",
  "#5fb3b3",
  "#d89288",
  "#9bb29d",
  "#c8a45f",
];

export function laneColor(lane: number): string {
  return PALETTE[lane % PALETTE.length];
}

/** Splits `%D`-style decoration tokens into typed, display-ready badges. */
export function classifyRefs(refs: string[]): RefBadge[] {
  const badges: RefBadge[] = [];
  for (const raw of refs) {
    if (raw.startsWith("tag: ")) {
      badges.push({ label: raw.slice(5), kind: "tag" });
      continue;
    }
    const parts = raw.split(" -> ");
    const name = parts.length > 1 ? parts[1] : parts[0];
    const isHead = parts.length > 1 && parts[0] === "HEAD";
    badges.push({
      label: name,
      kind: isHead ? "head" : name.includes("/") ? "remote" : "branch",
    });
  }
  return badges;
}
