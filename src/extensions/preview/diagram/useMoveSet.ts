import { useEffect, useMemo, useRef, useState } from "react";
import { moveOps, type MoveSet } from "./boardMath.ts";
import type { Diagram, DiagramOp, Point } from "./types.ts";

const IDLE_MS = 400;

export type Shift = { set: MoveSet; dx: number; dy: number };

/** A selection moving together: the live preview of a drag or of arrow-key
 * nudges, and the one op per item written when it ends. */
export function useMoveSet(
  latest: React.RefObject<{ diagram: Diagram; pos: Map<string, Point> }>,
  pushAll: (ops: DiagramOp[]) => void,
) {
  const [shift, setShift] = useState<Shift | null>(null);
  const timer = useRef(0);
  const nudged = useRef<Shift | null>(null);

  const commit = (s: Shift) => {
    const { diagram, pos } = latest.current;
    pushAll(moveOps(diagram, pos, s.set, s.dx, s.dy));
    setShift(null);
  };
  const flush = () => {
    window.clearTimeout(timer.current);
    const s = nudged.current;
    nudged.current = null;
    if (s) commit(s);
  };
  const flushRef = useRef(flush);
  flushRef.current = flush;
  useEffect(() => () => flushRef.current(), []);

  const nudge = (set: MoveSet, dx: number, dy: number) => {
    const at = nudged.current;
    const next = {
      set: at?.set ?? set,
      dx: (at?.dx ?? 0) + dx,
      dy: (at?.dy ?? 0) + dy,
    };
    nudged.current = next;
    setShift(next);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(flush, IDLE_MS);
  };

  const moving = useMemo(() => {
    const ids = new Set<string>();
    if (!shift || (!shift.dx && !shift.dy)) return ids;
    for (const id of shift.set.nodes) ids.add(id);
    return ids;
  }, [shift]);

  return { shift, setShift, commit, nudge, moving };
}
