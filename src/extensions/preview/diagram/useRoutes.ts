import { useMemo } from "react";
import {
  groupLabelRect,
  groupRects,
  labelAt,
  nodeSize,
  routeEdge,
  routeEdges,
} from "./model.ts";
import type { Diagram, DiagramEdge, Point, Rect } from "./types.ts";

export type Routed = {
  edge: DiagramEdge;
  points: Point[];
  label: ReturnType<typeof labelAt>;
};

const SLIDE = [0.5, 0.25, 0.75, 0.1, 0.9];
const EXIT_AT = [28, 24, 32];

const overlaps = (a: Rect, b: Rect) =>
  a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

const chipAt = (x: number, y: number, vertical: boolean, w: number): Rect =>
  vertical
    ? { x: x + 6, y: y - 8, w, h: 16 }
    : { x: x - w / 2, y: y - 22, w, h: 16 };

/** Where an edge's label chip goes: the midpoint of its longest run, slid
 * along that run to the first spot clear of every block. A label on an edge
 * that leaves a diamond sits just after the vertex instead, beside the first
 * run, so it reads as the answer to the question. */
function labelSpot(
  points: Point[],
  text: string | undefined,
  blocks: Rect[],
  exit: boolean,
): ReturnType<typeof labelAt> {
  const mid = labelAt(points);
  if (!text || points.length < 2) return mid;
  if (exit) {
    const [a, b] = points;
    const vertical = a.x === b.x;
    const len = Math.abs(b.x - a.x) + Math.abs(b.y - a.y);
    const w = text.length * 6 + 10;
    for (const d of EXIT_AT) {
      const t = Math.min(d, len - 4) / len;
      const x = a.x + (b.x - a.x) * t;
      const y = a.y + (b.y - a.y) * t;
      if (!blocks.some((r) => overlaps(chipAt(x, y, vertical, w), r)))
        return { x, y, vertical };
    }
  }
  let best = 0;
  let bestLen = -1;
  for (let i = 0; i < points.length - 1; i++) {
    const len =
      Math.abs(points[i + 1].x - points[i].x) +
      Math.abs(points[i + 1].y - points[i].y);
    if (len > bestLen) [best, bestLen] = [i, len];
  }
  const p = points[best];
  const q = points[best + 1];
  const w = text.length * 6 + 10;
  for (const t of SLIDE) {
    const x = p.x + (q.x - p.x) * t;
    const y = p.y + (q.y - p.y) * t;
    const chip = chipAt(x, y, mid.vertical, w);
    if (!blocks.some((b) => overlaps(chip, b)))
      return { x, y, vertical: mid.vertical };
  }
  return mid;
}

/** Routes of the visible edges. The whole set is routed together and kept while
 * nothing moves; while blocks are dragged only the edges touching them are
 * re-routed, one by one, and the full routing runs again on drop. */
export function useRoutes(
  diagram: Diagram,
  shown: Diagram,
  pos: Map<string, Point>,
  shownPos: Map<string, Point>,
  groups: Map<string, Rect>,
  moving: Set<string>,
): Routed[] {
  const still = useMemo(() => groupRects(diagram), [diagram]);
  const base = useMemo(() => {
    const avoid = shown.groups.flatMap((g) => {
      const r = still.get(g.id);
      return r ? [groupLabelRect(r, g.label)] : [];
    });
    return routeEdges(shown, pos, avoid);
  }, [shown, pos, still]);
  const nodes = useMemo(
    () => new Map(shown.nodes.map((n) => [n.id, n])),
    [shown],
  );
  const live = moving.size
    ? shown.groups.flatMap((g) => {
        const r = groups.get(g.id);
        return r ? [groupLabelRect(r, g.label)] : [];
      })
    : [];
  const rectOf = (id: string): Rect | null => {
    const at = shownPos.get(id);
    const node = nodes.get(id);
    return at && node ? { x: at.x, y: at.y, ...nodeSize(node) } : null;
  };
  const blocks = shown.nodes.flatMap((n) => {
    const at = shownPos.get(n.id);
    return at ? [{ ...at, ...nodeSize(n) }] : [];
  });
  return shown.edges.flatMap((edge) => {
    let points = base.get(edge.id);
    if (moving.has(edge.from) || moving.has(edge.to)) {
      const a = rectOf(edge.from);
      const b = rectOf(edge.to);
      points = a && b ? routeEdge(a, b, live) : points;
    }
    return points
      ? [
          {
            edge,
            points,
            label: labelSpot(
              points,
              edge.label,
              blocks,
              nodes.get(edge.from)?.shape === "diamond",
            ),
          },
        ]
      : [];
  });
}
