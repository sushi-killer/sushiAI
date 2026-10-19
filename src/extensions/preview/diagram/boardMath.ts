import { MAX_COORD, groupRects, nodeSize, placeNodes } from "./model.ts";
import type { Diagram, DiagramOp, Picked, Point, Rect } from "./types.ts";

export const NOTE_W = 280;
export const NOTE_H = 30;
export const GROUP_W = 420;
export const GROUP_H = 240;
const GAP = 40;
export const GROUP_HEAD_H = 26;

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
  const rects: Rect[] = [...groupRects(d, pos).values()];
  for (const n of d.nodes) {
    const at = pos.get(n.id);
    if (at) rects.push({ ...at, ...nodeSize(n) });
  }
  for (const n of d.notes) rects.push({ x: n.x, y: n.y, w: NOTE_W, h: NOTE_H });
  if (!rects.length) return clampSpot({ x: 0, y: 0 }, w, h);
  const x = Math.min(...rects.map((r) => r.x));
  const y = Math.max(...rects.map((r) => r.y + r.h)) + GAP;
  return clampSpot({ x: snap(x), y: snap(y) }, w, h);
}

/** The smallest boundary whose rect contains `p`, or "". */
export function groupAt(rects: Map<string, Rect>, p: Point): string {
  let best = "";
  let area = Infinity;
  for (const [id, r] of rects) {
    const inside =
      p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h;
    if (inside && r.w * r.h < area) {
      best = id;
      area = r.w * r.h;
    }
  }
  return best;
}

/** Boundary rects as they would be with component `id` out of the picture, so
 * a wrapping boundary does not stretch to follow the dragged component. */
export function dropRects(
  d: Diagram,
  pos: Map<string, Point>,
  id: string,
): Map<string, Rect> {
  return groupRects({ ...d, nodes: d.nodes.filter((n) => n.id !== id) }, pos);
}

/** The boundary component `id` belongs to after being dropped with its
 * top-left at `at`. Its own boundary counts as drawn before the drag, so a
 * nudge keeps it; a smaller boundary under the drop point wins. */
export function dropGroup(
  d: Diagram,
  pos: Map<string, Point>,
  id: string,
  at: Point,
): string {
  const rects = dropRects(d, pos, id);
  const own = d.nodes.find((n) => n.id === id)?.group;
  const before = own ? groupRects(d, pos).get(own) : undefined;
  if (own && before) rects.set(own, before);
  const { w, h } = nodeSize(d.nodes.find((n) => n.id === id) ?? {});
  return groupAt(rects, { x: at.x + w / 2, y: at.y + h / 2 });
}

/** Writes the explicit bounds of a boundary that has none when its last
 * member leaves it (moved out, regrouped or deleted), so it stays on the
 * board. `next` is the group the component goes to, "" for none. */
export function leaveBounds(
  d: Diagram,
  pos: Map<string, Point>,
  id: string,
  next: string,
): DiagramOp[] {
  const own = d.nodes.find((n) => n.id === id)?.group;
  const group = own ? d.groups.find((g) => g.id === own) : undefined;
  if (!own || !group || own === next) return [];
  const bounded = [group.x, group.y, group.w, group.h].every(
    (v) => typeof v === "number",
  );
  const others = d.nodes.some((n) => n.group === own && n.id !== id);
  const rect = groupRects(d, pos).get(own);
  if (bounded || others || !rect) return [];
  return [{ op: "resize-group", id: own, rect }];
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
  for (const [id, r] of groupRects(d, pos))
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
    if (intersects(box, { x: n.x, y: n.y, w: NOTE_W, h: NOTE_H }))
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

/** What moves with a selection: a node whose group is picked too moves with
 * the group, not by itself. Edges do not move. */
export function moveSet(d: Diagram, picks: Picked[]): MoveSet {
  const ids = (type: Picked["type"]) =>
    picks.filter((p) => p.type === type).map((p) => p.id);
  const groups = ids("group");
  const inGroup = new Set(groups);
  const nodes = ids("node").filter((id) => {
    const g = d.nodes.find((n) => n.id === id)?.group;
    return !(g && inGroup.has(g));
  });
  return { nodes, notes: ids("note"), groups };
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
  const rects = groupRects(d, pos);
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

/** Explicit bounds for each auto-sized boundary that loses all its members,
 * so it stays on the board. `leaving` are the nodes going away; boundaries in
 * `keep` are going away or receiving them, so they need none. */
export function emptiedBounds(
  d: Diagram,
  pos: Map<string, Point>,
  leaving: Set<string>,
  keep: Set<string>,
): DiagramOp[] {
  const rects = groupRects(d, pos);
  const ops: DiagramOp[] = [];
  for (const g of d.groups) {
    const members = d.nodes.filter((n) => n.group === g.id);
    const bounded = [g.x, g.y, g.w, g.h].every((v) => typeof v === "number");
    const rect = rects.get(g.id);
    if (
      rect &&
      !bounded &&
      !keep.has(g.id) &&
      members.length &&
      members.every((n) => leaving.has(n.id))
    )
      ops.push({ op: "resize-group", id: g.id, rect });
  }
  return ops;
}

/** The ops that delete `picks`: links first, then blocks, notes and boundaries. */
export function deleteOps(
  d: Diagram,
  pos: Map<string, Point>,
  picks: Picked[],
): DiagramOp[] {
  const ids = (type: Picked["type"]) =>
    picks.filter((p) => p.type === type).map((p) => p.id);
  const nodes = ids("node");
  const groups = ids("group");
  return [
    ...emptiedBounds(d, pos, new Set(nodes), new Set(groups)),
    ...[...ids("edge"), ...nodes, ...ids("note"), ...groups].map(
      (id): DiagramOp => ({ op: "delete", id }),
    ),
  ];
}
