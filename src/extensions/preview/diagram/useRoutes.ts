import { useMemo } from "react";
import {
  groupLabelRect,
  groupRects,
  nodeSize,
  placeLabels,
  routeEdge,
  routeEdges,
  type LabelSpot,
} from "./model.ts";
import type { Diagram, DiagramEdge, Point, Rect, Size } from "./types.ts";

export type Routed = {
  edge: DiagramEdge;
  points: Point[];
  label: LabelSpot;
};

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
  sizes: Map<string, Size>,
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
    return at && node
      ? { x: at.x, y: at.y, ...(sizes.get(id) ?? nodeSize(node)) }
      : null;
  };
  const blocks = shown.nodes.flatMap((n) => {
    const at = shownPos.get(n.id);
    return at ? [{ ...at, ...(sizes.get(n.id) ?? nodeSize(n)) }] : [];
  });
  const routes = shown.edges.flatMap((edge) => {
    let points = base.get(edge.id);
    if (moving.has(edge.from) || moving.has(edge.to)) {
      const a = rectOf(edge.from);
      const b = rectOf(edge.to);
      points = a && b ? routeEdge(a, b, live) : points;
    }
    return points ? [{ edge, points }] : [];
  });
  const labels = placeLabels(
    routes.map(({ edge, points }) => ({
      id: edge.id,
      text: edge.label,
      points,
      arrow: edge.arrow,
      exit: nodes.get(edge.from)?.shape === "diamond",
    })),
    blocks,
  );
  return routes.map(({ edge, points }) => ({
    edge,
    points,
    label: labels.get(edge.id) as LabelSpot,
  }));
}
