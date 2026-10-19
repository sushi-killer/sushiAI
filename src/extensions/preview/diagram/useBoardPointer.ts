import { useRef, useState } from "react";
import {
  marqueeHits,
  moveSet,
  samePick,
  snap,
  type Gesture,
  type MoveSet,
} from "./boardMath.ts";
import { groupRects, MIN_GROUP_H, MIN_GROUP_W, newId } from "./model.ts";
import type { Diagram, DiagramOp, Picked, Point, Rect } from "./types.ts";
import type { useMarquee } from "./useMarquee.ts";
import type { useMoveSet } from "./useMoveSet.ts";
import type { useSelection } from "./useSelection.ts";
import type { View } from "./useViewport.ts";

const multi = (
  item: Picked,
  shift: boolean,
  set: MoveSet,
  event: React.PointerEvent,
): Gesture => ({
  kind: "multi",
  item,
  shift,
  set,
  sx: event.clientX,
  sy: event.clientY,
  last: null,
});

type Args = {
  boardRef: React.RefObject<HTMLDivElement | null>;
  latest: React.RefObject<{ diagram: Diagram; pos: Map<string, Point> }>;
  selection: ReturnType<typeof useSelection>;
  mover: ReturnType<typeof useMoveSet>;
  marquee: ReturnType<typeof useMarquee>;
  viewRef: React.RefObject<View>;
  setView: (view: View) => void;
  boardPoint: (event: { clientX: number; clientY: number }) => Point;
  push: (op: DiagramOp, immediate?: boolean) => void;
  spaceDown: boolean;
};

/** Everything a pointer press on the board can start: pan, marquee, a drag of
 * one item or of the whole selection, a boundary resize, a new link. */
export function useBoardPointer(a: Args) {
  const { boardRef, latest, selection, mover, marquee, viewRef, push } = a;
  const { pick, clickOn, addAll, setSel } = selection;
  const [drag, setDrag] = useState<{ id: string; x: number; y: number } | null>(
    null,
  );
  const [gdrag, setGdrag] = useState<{
    id: string;
    rect: Rect;
    dx: number;
    dy: number;
  } | null>(null);
  const [rubber, setRubber] = useState<{ from: string; to: Point } | null>(
    null,
  );
  const gesture = useRef<Gesture | null>(null);
  const [panning, setPanning] = useState(false);

  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    const t = event.target as HTMLElement;
    if (t.closest(".dg-bar, .dg-rename, .dg-fab")) return;
    if (event.button !== 0 && event.button !== 1) return;
    const port = t.closest<HTMLElement>("[data-port]");
    const node = t.closest<HTMLElement>("[data-node]");
    const note = t.closest<HTMLElement>("[data-note]");
    const edge = t.closest<HTMLElement>("[data-edge]");
    const resize = t.closest<HTMLElement>("[data-resize]");
    const group = t.closest<HTMLElement>("[data-group]");
    const { diagram: d, pos: p } = latest.current;
    const gid = (resize || group)?.dataset[resize ? "resize" : "group"];
    const additive = event.shiftKey;
    boardRef.current?.focus();
    if (event.button === 1 || a.spaceDown) {
      event.preventDefault();
      const v = viewRef.current;
      gesture.current = {
        kind: "pan",
        sx: event.clientX,
        sy: event.clientY,
        tx: v.tx,
        ty: v.ty,
        moved: false,
      };
    } else if (gid) {
      const base = groupRects(d).get(gid);
      const item: Picked = { type: "group", id: gid };
      const next = resize ? [item] : clickOn(item, additive);
      if (resize) pick(item);
      if (!base || !next.some((q) => samePick(q, item))) return;
      gesture.current =
        next.length > 1
          ? multi(item, additive, moveSet(d, next), event)
          : {
              kind: "group",
              mode: resize ? "resize" : "move",
              id: gid,
              sx: event.clientX,
              sy: event.clientY,
              base,
              last: null,
            };
    } else if (port) {
      const from = port.dataset.port!;
      pick({ type: "node", id: from });
      gesture.current = { kind: "connect", from };
    } else if (node || note) {
      const id = (node || note)!.dataset[node ? "node" : "note"]!;
      const at = node ? p.get(id) : d.notes.find((n) => n.id === id);
      const item: Picked = { type: node ? "node" : "note", id };
      const next = clickOn(item, additive);
      if (!at || !next.some((q) => samePick(q, item))) return;
      gesture.current =
        next.length > 1
          ? multi(item, additive, moveSet(d, next), event)
          : {
              kind: "move",
              type: item.type as "node" | "note",
              id,
              sx: event.clientX,
              sy: event.clientY,
              ox: at.x,
              oy: at.y,
              last: null,
            };
    } else if (edge) {
      clickOn({ type: "edge", id: edge.dataset.edge! }, additive);
      return;
    } else {
      if (!additive) pick(null);
      marquee.start(event, (box) => {
        if (!box) return;
        const hits = marqueeHits(
          latest.current.diagram,
          latest.current.pos,
          box,
        );
        if (additive) addAll(hits);
        else setSel(hits);
      });
      return;
    }
    const onMove = (e: PointerEvent) => {
      const g = gesture.current;
      if (!g) return;
      const v = viewRef.current;
      if (g.kind === "connect")
        return setRubber({ from: g.from, to: a.boardPoint(e) });
      const dx = e.clientX - g.sx;
      const dy = e.clientY - g.sy;
      if (g.kind === "pan") {
        if (!g.moved && Math.hypot(dx, dy) <= 3) return;
        g.moved = true;
        setPanning(true);
        return a.setView({ ...v, tx: g.tx + dx, ty: g.ty + dy });
      }
      if (g.kind === "multi") {
        if (!g.last && Math.hypot(dx, dy) <= 3) return;
        g.last = { x: snap(dx / v.s), y: snap(dy / v.s) };
        return mover.setShift({ set: g.set, dx: g.last.x, dy: g.last.y });
      }
      if (g.kind === "group") {
        if (!g.last && Math.hypot(dx, dy) <= 3) return;
        const ox = snap(dx / v.s);
        const oy = snap(dy / v.s);
        const { base } = g;
        g.last =
          g.mode === "move"
            ? {
                rect: { ...base, x: base.x + ox, y: base.y + oy },
                dx: ox,
                dy: oy,
              }
            : {
                rect: {
                  ...base,
                  w: Math.max(MIN_GROUP_W, snap(base.w + dx / v.s)),
                  h: Math.max(MIN_GROUP_H, snap(base.h + dy / v.s)),
                },
                dx: 0,
                dy: 0,
              };
        return setGdrag({ id: g.id, ...g.last });
      }
      if (!g.last && Math.hypot(dx, dy) <= 3) return;
      g.last = { x: snap(g.ox + dx / v.s), y: snap(g.oy + dy / v.s) };
      setDrag({ id: g.id, ...g.last });
    };
    const onUp = (e: PointerEvent) => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      const g = gesture.current;
      gesture.current = null;
      if (!g) return;
      if (g.kind === "pan") setPanning(false);
      if (g.kind === "multi") {
        if (g.last) mover.commit({ set: g.set, dx: g.last.x, dy: g.last.y });
        else if (!g.shift) pick(g.item);
      }
      if (g.kind === "move" && g.last) {
        push({ op: "move", id: g.id, ...g.last }, true);
        setDrag(null);
      }
      if (g.kind === "group" && g.last) {
        const { rect, dx, dy } = g.last;
        push(
          g.mode === "move"
            ? { op: "move-group", id: g.id, rect, dx, dy }
            : { op: "resize-group", id: g.id, rect },
          true,
        );
        setGdrag(null);
      }
      if (g.kind === "connect") {
        setRubber(null);
        const to = document
          .elementFromPoint(e.clientX, e.clientY)
          ?.closest<HTMLElement>("[data-node]")?.dataset.node;
        if (to && to !== g.from)
          push({
            op: "connect",
            edge: {
              id: newId(latest.current.diagram, `${g.from}-${to}`),
              from: g.from,
              to,
            },
          });
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  return { drag, gdrag, rubber, panning, onPointerDown };
}
