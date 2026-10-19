import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { NOTE_H, NOTE_W } from "./boardMath.ts";
import { groupRects, nodeSize } from "./model.ts";
import type { Diagram, Point, Rect } from "./types.ts";

export type View = { tx: number; ty: number; s: number };

const MIN_ZOOM = 0.4;
const MAX_ZOOM = 2;
const FIT_PAD = 24;

/** Pan and zoom of the board, plus fit-to-content and screen-to-board mapping. */
export function useViewport(
  boardRef: React.RefObject<HTMLDivElement | null>,
  latest: React.RefObject<{ diagram: Diagram; pos: Map<string, Point> }>,
) {
  const [view, setView] = useState<View>({ tx: FIT_PAD, ty: FIT_PAD, s: 1 });
  const viewRef = useRef(view);
  viewRef.current = view;

  const fit = useCallback(() => {
    const el = boardRef.current;
    if (!el) return;
    const { diagram: d, pos: p } = latest.current;
    const rects: Rect[] = [...groupRects(d, p).values()];
    for (const n of d.nodes) {
      const at = p.get(n.id);
      if (at) rects.push({ ...at, ...nodeSize(n) });
    }
    for (const n of d.notes)
      rects.push({ x: n.x, y: n.y, w: NOTE_W, h: NOTE_H });
    if (!rects.length) return setView({ tx: FIT_PAD, ty: FIT_PAD, s: 1 });
    const x0 = Math.min(...rects.map((r) => r.x));
    const y0 = Math.min(...rects.map((r) => r.y));
    const w = Math.max(...rects.map((r) => r.x + r.w)) - x0;
    const h = Math.max(...rects.map((r) => r.y + r.h)) - y0;
    const s = Math.min(
      1,
      (el.clientWidth - 2 * FIT_PAD) / w,
      (el.clientHeight - 2 * FIT_PAD) / h,
    );
    const k = Math.max(s, MIN_ZOOM);
    setView({
      tx: Math.max(FIT_PAD, (el.clientWidth - w * k) / 2) - x0 * k,
      ty: Math.max(FIT_PAD, (el.clientHeight - h * k) / 2) - y0 * k,
      s: k,
    });
  }, [boardRef, latest]);
  useLayoutEffect(fit, [fit]);

  // Wheel and trackpad: pan, or zoom around the cursor with ctrl/cmd and pinch.
  useEffect(() => {
    const el = boardRef.current;
    if (!el) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const v = viewRef.current;
      if (!(event.ctrlKey || event.metaKey))
        return setView({
          ...v,
          tx: v.tx - event.deltaX,
          ty: v.ty - event.deltaY,
        });
      const box = el.getBoundingClientRect();
      const cx = event.clientX - box.left;
      const cy = event.clientY - box.top;
      const s = Math.min(
        MAX_ZOOM,
        Math.max(MIN_ZOOM, v.s * Math.exp(-event.deltaY * 0.01)),
      );
      setView({
        s,
        tx: cx - ((cx - v.tx) * s) / v.s,
        ty: cy - ((cy - v.ty) * s) / v.s,
      });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [boardRef]);

  const boardPoint = (event: { clientX: number; clientY: number }): Point => {
    const box = boardRef.current!.getBoundingClientRect();
    const v = viewRef.current;
    return {
      x: (event.clientX - box.left - v.tx) / v.s,
      y: (event.clientY - box.top - v.ty) / v.s,
    };
  };

  return { view, viewRef, setView, fit, boardPoint };
}
