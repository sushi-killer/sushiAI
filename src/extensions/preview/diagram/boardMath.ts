import {
  MAX_COORD,
  groupAt,
  groupRects,
  nodeSize,
  noteSize,
  placeNodes,
  within,
} from "./model.ts";
import {
  type Diagram,
  type DiagramOp,
  type Picked,
  type Point,
  type Rect,
} from "./types.ts";

export const GROUP_W = 420;
export const GROUP_H = 240;
const GAP = 40;
export const GROUP_HEAD_H = 26;

/** "1 block · 2 links": the status bar's count, singular for one. */
export function countLabel(blocks: number, links: number): string {
  const n = (count: number, word: string) =>
    `${count} ${word}${count === 1 ? "" : "s"}`;
  return `${n(blocks, "block")} · ${n(links, "link")}`;
}

export const snap = (n: number) => Math.round(n / 2) * 2;

/** `p` moved inside the range a file may hold for an item of `w` x `h`. */
export function clampSpot(p: Point, w: number, h: number): Point {
  const fit = (v: number, size: number) =>
    Math.min(Math.max(v, -MAX_COORD), MAX_COORD - size);
  return { x: fit(p.x, w), y: fit(p.y, h) };
}

/** Top-left for a new item: below everything already drawn. */
export function freeSpot(d: Diagram, w: number, h: number): Point {
  const pos = placeNodes(d);
  const rects: Rect[] = [...groupRects(d).values()];
  for (const n of d.nodes) {
    const at = pos.get(n.id);
    if (at) rects.push({ ...at, ...nodeSize(n) });
  }
  for (const n of d.notes) rects.push({ x: n.x, y: n.y, ...noteSize(n) });
  if (!rects.length) return clampSpot({ x: 0, y: 0 }, w, h);
  const x = Math.min(...rects.map((r) => r.x));
  const y = Math.max(...rects.map((r) => r.y + r.h)) + GAP;
  return clampSpot({ x: snap(x), y: snap(y) }, w, h);
}

/** The boundary component `id` belongs to after being dropped with its
 * top-left at `at`: the smallest one around its centre. */
export function dropGroup(d: Diagram, id: string, at: Point): string {
  const { w, h } = nodeSize(d.nodes.find((n) => n.id === id) ?? {});
  return groupAt(groupRects(d), { x: at.x + w / 2, y: at.y + h / 2 });
}

export type Gesture =
  | {
      kind: "pan";
      sx: number;
      sy: number;
      tx: number;
      ty: number;
      moved: boolean;
    }
  | {
      kind: "move";
      type: "node" | "note";
      id: string;
      sx: number;
      sy: number;
      ox: number;
      oy: number;
      last: Point | null;
    }
  | {
      kind: "group";
      mode: "move" | "resize";
      id: string;
      sx: number;
      sy: number;
      base: Rect;
      last: { rect: Rect; dx: number; dy: number } | null;
    }
  | {
      kind: "multi";
      item: Picked;
      shift: boolean;
      set: MoveSet;
      sx: number;
      sy: number;
      last: Point | null;
    }
  | {
      kind: "size";
      id: string;
      sx: number;
      sy: number;
      base: { w: number; h: number };
      last: { w: number; h: number } | null;
    }
  | { kind: "connect"; from: string };

export const typing = (el: EventTarget | null) =>
  el instanceof HTMLElement && /^(INPUT|SELECT|TEXTAREA)$/.test(el.tagName);

export const intersects = (a: Rect, b: Rect) =>
  a.x <= b.x + b.w && b.x <= a.x + a.w && a.y <= b.y + b.h && b.y <= a.y + a.h;

export const samePick = (a: Picked, b: Picked) =>
  a.type === b.type && a.id === b.id;

/** Every node, note and group the marquee touches (a group by its header
 * strip, the part that can be clicked), then the edges between two picked
 * nodes. */
export function marqueeHits(
  d: Diagram,
  pos: Map<string, Point>,
  box: Rect,
): Picked[] {
  const out: Picked[] = [];
  for (const [id, r] of groupRects(d))
    if (intersects(box, { ...r, h: Math.min(r.h, GROUP_HEAD_H) }))
      out.push({ type: "group", id });
  const nodes = new Set<string>();
  for (const n of d.nodes) {
    const at = pos.get(n.id);
    if (at && intersects(box, { ...at, ...nodeSize(n) })) {
      nodes.add(n.id);
      out.push({ type: "node", id: n.id });
    }
  }
  for (const n of d.notes)
    if (intersects(box, { x: n.x, y: n.y, ...noteSize(n) }))
      out.push({ type: "note", id: n.id });
  for (const e of d.edges)
    if (nodes.has(e.from) && nodes.has(e.to))
      out.push({ type: "edge", id: e.id });
  return out;
}

export function allPicks(d: Diagram): Picked[] {
  return [
    ...d.groups.map((g): Picked => ({ type: "group", id: g.id })),
    ...d.nodes.map((n): Picked => ({ type: "node", id: n.id })),
    ...d.notes.map((n): Picked => ({ type: "note", id: n.id })),
    ...d.edges.map((e): Picked => ({ type: "edge", id: e.id })),
  ];
}

/** `picks` without what the file no longer has. */
export function livePicks(d: Diagram, picks: Picked[]): Picked[] {
  const has: Record<Picked["type"], Set<string>> = {
    node: new Set(d.nodes.map((n) => n.id)),
    edge: new Set(d.edges.map((e) => e.id)),
    note: new Set(d.notes.map((n) => n.id)),
    group: new Set(d.groups.map((g) => g.id)),
  };
  return picks.filter((p) => has[p.type].has(p.id));
}

export type MoveSet = { nodes: string[]; notes: string[]; groups: string[] };

/** What moves with a selection: whatever lies inside a picked group moves
 * with the group, not by itself. Edges do not move. */
export function moveSet(d: Diagram, picks: Picked[]): MoveSet {
  const ids = (type: Picked["type"]) =>
    picks.filter((p) => p.type === type).map((p) => p.id);
  const carried = new Set(ids("group").flatMap((g) => [...within(d, g)]));
  const free = (id: string) => !carried.has(id);
  return {
    nodes: ids("node").filter(free),
    notes: ids("note").filter(free),
    groups: ids("group").filter(free),
  };
}

/** One op per moved item, for a shift of (dx, dy). */
export function moveOps(
  d: Diagram,
  pos: Map<string, Point>,
  set: MoveSet,
  dx: number,
  dy: number,
): DiagramOp[] {
  if (!dx && !dy) return [];
  const ops: DiagramOp[] = [];
  const rects = groupRects(d);
  for (const id of set.groups) {
    const r = rects.get(id);
    if (r)
      ops.push({
        op: "move-group",
        id,
        rect: { ...r, x: r.x + dx, y: r.y + dy },
        dx,
        dy,
      });
  }
  for (const id of set.nodes) {
    const at = pos.get(id);
    if (at) ops.push({ op: "move", id, x: at.x + dx, y: at.y + dy });
  }
  for (const id of set.notes) {
    const note = d.notes.find((n) => n.id === id);
    if (note) ops.push({ op: "move", id, x: note.x + dx, y: note.y + dy });
  }
  return ops;
}

/** The ops that delete `picks`: links first, then blocks, notes and boundaries. */
export function deleteOps(picks: Picked[]): DiagramOp[] {
  const ids = (type: Picked["type"]) =>
    picks.filter((p) => p.type === type).map((p) => p.id);
  return [...ids("edge"), ...ids("node"), ...ids("note"), ...ids("group")].map(
    (id): DiagramOp => ({ op: "delete", id }),
  );
}
