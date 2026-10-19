import { useCallback, useEffect, useMemo, useState } from "react";
import { allPicks, livePicks, samePick } from "./boardMath.ts";
import type { Diagram, Picked } from "./types.ts";

/** What is selected on the board. Ids follow a remap and vanish with the file. */
export function useSelection(diagram: Diagram) {
  const [raw, setRaw] = useState<Picked[]>([]);
  const sel = useMemo(() => livePicks(diagram, raw), [diagram, raw]);
  useEffect(() => {
    if (sel.length !== raw.length) setRaw(sel);
  }, [sel, raw]);

  const pick = useCallback(
    (one: Picked | null) => setRaw(one ? [one] : []),
    [],
  );
  const clickOn = useCallback(
    (item: Picked, additive: boolean): Picked[] => {
      const has = sel.some((p) => samePick(p, item));
      const next = additive
        ? has
          ? sel.filter((p) => !samePick(p, item))
          : [...sel, item]
        : has
          ? sel
          : [item];
      setRaw(next);
      return next;
    },
    [sel],
  );
  const addAll = useCallback(
    (hits: Picked[]) =>
      setRaw((cur) => [
        ...cur,
        ...hits.filter((h) => !cur.some((p) => samePick(p, h))),
      ]),
    [],
  );
  const remap = useCallback(
    (to: (id: string) => string) =>
      setRaw((cur) => cur.map((p) => ({ ...p, id: to(p.id) }))),
    [],
  );
  const selectAll = useCallback(() => setRaw(allPicks(diagram)), [diagram]);
  const isOn = useMemo(() => {
    const keys = new Set(sel.map((p) => `${p.type}:${p.id}`));
    return (type: Picked["type"], id: string) => keys.has(`${type}:${id}`);
  }, [sel]);

  return {
    sel,
    one: sel.length === 1 ? sel[0] : null,
    isOn,
    pick,
    setSel: setRaw,
    clickOn,
    addAll,
    remap,
    selectAll,
  };
}
