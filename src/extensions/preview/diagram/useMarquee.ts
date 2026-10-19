import { useState } from "react";
import type { Point, Rect } from "./types.ts";

/** The rubber-band rectangle: drawn in board pixels while the pointer is down,
 * handed back in board coordinates on release (null for a plain click). */
export function useMarquee(
  boardRef: React.RefObject<HTMLDivElement | null>,
  boardPoint: (event: { clientX: number; clientY: number }) => Point,
) {
  const [box, setBox] = useState<Rect | null>(null);
  const start = (
    event: { clientX: number; clientY: number },
    onEnd: (hit: Rect | null) => void,
  ) => {
    const sx = event.clientX;
    const sy = event.clientY;
    let moved = false;
    const onMove = (e: PointerEvent) => {
      if (!moved && Math.hypot(e.clientX - sx, e.clientY - sy) <= 3) return;
      moved = true;
      const at = boardRef.current?.getBoundingClientRect();
      if (!at) return;
      setBox({
        x: Math.min(sx, e.clientX) - at.left,
        y: Math.min(sy, e.clientY) - at.top,
        w: Math.abs(e.clientX - sx),
        h: Math.abs(e.clientY - sy),
      });
    };
    const onUp = (e: PointerEvent) => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      setBox(null);
      if (!moved) return onEnd(null);
      const a = boardPoint({ clientX: sx, clientY: sy });
      const b = boardPoint(e);
      onEnd({
        x: Math.min(a.x, b.x),
        y: Math.min(a.y, b.y),
        w: Math.abs(a.x - b.x),
        h: Math.abs(a.y - b.y),
      });
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };
  return { box, start };
}
