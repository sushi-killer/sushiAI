import { useState } from "react";
import {
  addItems,
  thingItems,
  type AddChoice,
  type MenuAt,
  type MenuItem,
} from "./ContextMenu.tsx";
import {
  GROUP_H,
  GROUP_W,
  NOTE_H,
  NOTE_W,
  clampSpot,
  freeSpot,
  snap,
} from "./boardMath.ts";
import { quoteOf } from "./Inspect.tsx";
import { quoteMany } from "./InspectMany.tsx";
import { newId, nodeSize } from "./model.ts";
import type { Diagram, DiagramOp, Picked, Point } from "./types.ts";

type Args = {
  diagram: Diagram;
  push: (op: DiagramOp, immediate?: boolean) => void;
  boardRef: React.RefObject<HTMLDivElement | null>;
  boardPoint: (event: { clientX: number; clientY: number }) => Point;
  reveal: React.MutableRefObject<boolean>;
  sel: Picked[];
  setPicked: (picked: Picked | null) => void;
  setRenaming: (id: string) => void;
  setFresh: (id: string) => void;
  remove: (picks: Picked[]) => void;
  onComment: (quote: string, where: string) => void;
};

/** The right-click menu and the "+" menu: what each offers and what it does. */
export function useBoardMenu(a: Args) {
  const {
    diagram,
    push,
    boardRef,
    boardPoint,
    reveal,
    sel,
    setPicked,
    setRenaming,
    setFresh,
    remove,
  } = a;
  const [menu, setMenu] = useState<{ at: MenuAt; items: MenuItem[] } | null>(
    null,
  );
  const add = (choice: AddChoice, at?: Point) => {
    const spot = (w: number, h: number) => {
      if (at) return clampSpot(at, w, h);
      reveal.current = true;
      return freeSpot(diagram, w, h);
    };
    if (choice.what === "block") {
      const { preset } = choice;
      const id = newId(diagram, preset.id);
      const node = { id, title: preset.label, ...preset.node };
      push({
        op: "add-node",
        node: { ...node, ...spot(nodeSize(node).w, nodeSize(node).h) },
      });
      setPicked({ type: "node", id });
      setRenaming(id);
    } else if (choice.what === "group") {
      const id = newId(diagram, "group");
      push({
        op: "add-group",
        group: {
          id,
          label: "Group",
          icon: "server",
          ...spot(GROUP_W, GROUP_H),
          w: GROUP_W,
          h: GROUP_H,
        },
      });
      setPicked({ type: "group", id });
      setRenaming(id);
    } else {
      const id = newId(diagram, "note");
      push({
        op: "add-note",
        note: { id, text: "Question?", ...spot(NOTE_W, NOTE_H) },
      });
      setPicked({ type: "note", id });
      setFresh(id);
    }
  };
  const closeMenu = () => {
    setMenu(null);
    boardRef.current?.focus();
  };
  const onContextMenu = (event: React.MouseEvent<HTMLDivElement>) => {
    event.preventDefault();
    const t = event.target as HTMLElement;
    if (t.closest(".dg-rename, .dg-fab")) return;
    const at: MenuAt = { x: event.clientX, y: event.clientY };
    const hit = (attr: string) =>
      t.closest<HTMLElement>(`[data-${attr}]`)?.dataset[attr];
    const id =
      hit("node") ||
      hit("note") ||
      hit("edge") ||
      hit("group") ||
      hit("resize");
    if (!id) {
      const p = boardPoint(event);
      setPicked(null);
      return setMenu({
        at,
        items: addItems((choice) =>
          add(choice, { x: snap(p.x), y: snap(p.y) }),
        ),
      });
    }
    const type = hit("node")
      ? "node"
      : hit("note")
        ? "note"
        : hit("edge")
          ? "edge"
          : "group";
    const thing: Picked = { type, id };
    if (sel.length > 1 && sel.some((p) => p.type === type && p.id === id))
      return setMenu({
        at,
        items: thingItems({
          comment: () =>
            a.onComment(quoteMany(diagram, sel), `${sel.length} selected`),
          remove: () => remove(sel),
        }),
      });
    const node = diagram.nodes.find((n) => n.id === id);
    const edge = diagram.edges.find((e) => e.id === id);
    const proposed = node?.status === "proposed" || edge?.status === "proposed";
    setPicked(thing);
    setMenu({
      at,
      items: thingItems({
        onRename:
          type === "node" || type === "group"
            ? () => setRenaming(id)
            : undefined,
        edit: type === "note" ? () => setFresh(id) : undefined,
        comment: () => a.onComment(quoteOf(diagram, thing), `${type} ${id}`),
        accept: proposed ? () => push({ op: "accept", id }) : undefined,
        reject: proposed
          ? () => {
              push({ op: "reject", id });
              setPicked(null);
            }
          : undefined,
        remove: () => remove([thing]),
      }),
    });
  };
  const openAddMenu = (event: React.MouseEvent<HTMLButtonElement>) => {
    const box = event.currentTarget.getBoundingClientRect();
    setMenu({
      at: { x: box.left, y: box.top, above: true },
      items: addItems((choice) => add(choice)),
    });
  };

  return { menu, closeMenu, onContextMenu, openAddMenu };
}
