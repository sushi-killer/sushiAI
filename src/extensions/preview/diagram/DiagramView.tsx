import { Plus } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Toolbar } from "./Toolbar.tsx";
import { useBoardMenu } from "./useBoardMenu.ts";
import { useBoardPointer } from "./useBoardPointer.ts";
import { shownPath } from "../artifact.ts";
import { ContextMenu } from "./ContextMenu.tsx";
import {
  deleteOps,
  dropGroup,
  moveSet,
  typing,
  countLabel,
} from "./boardMath.ts";
import { Inspect } from "./Inspect.tsx";
import { InspectMany } from "./InspectMany.tsx";
import { BoardItems, BoardSvg, HeadsSvg, StatusText } from "./Layers.tsx";
import { groupRects, nodeSize, placeNodes, within } from "./model.ts";
import type { Diagram, DiagramOp, Picked, Point, Rect } from "./types.ts";
import { useDiagramSave, type SaveStatus } from "./useDiagramSave.ts";
import { useMarquee } from "./useMarquee.ts";
import { useMoveSet } from "./useMoveSet.ts";
import { useRoutes } from "./useRoutes.ts";
import { useSelection } from "./useSelection.ts";
import { useViewport } from "./useViewport.ts";

type Props = {
  text: string;
  hash: string;
  path: string;
  cwd: string;
  connection: string | undefined;
  onComment: (quote: string, where: string) => void;
  onEdited: () => void;
  readError?: string;
};

const NUDGE = 2;
const NUDGE_FAR = 10;
const ARROWS: Record<string, Point> = {
  ArrowLeft: { x: -1, y: 0 },
  ArrowRight: { x: 1, y: 0 },
  ArrowUp: { x: 0, y: -1 },
  ArrowDown: { x: 0, y: 1 },
};

function Board(
  props: Props & {
    onRemapRef: React.MutableRefObject<(remap: Map<string, string>) => void>;
    diagram: Diagram;
    status: SaveStatus;
    push: (op: DiagramOp, immediate?: boolean) => void;
  },
) {
  const { diagram, push } = props;
  const boardRef = useRef<HTMLDivElement>(null);
  const selection = useSelection(diagram);
  const { sel, one, isOn, pick } = selection;
  const [renaming, setRenaming] = useState("");
  const [fresh, setFresh] = useState("");
  const reveal = useRef(false);
  const [spaceDown, setSpaceDown] = useState(false);

  const pos = useMemo(() => placeNodes(diagram), [diagram]);
  const latest = useRef({ diagram, pos });
  latest.current = { diagram, pos };
  const pushAll = (ops: DiagramOp[]) =>
    ops.forEach((op, i) => push(op, i === ops.length - 1));
  const mover = useMoveSet(latest, pushAll);
  const { shift } = mover;
  const { view, viewRef, setView, fit, boardPoint } = useViewport(
    boardRef,
    latest,
  );
  const marquee = useMarquee(boardRef, boardPoint);
  const { drag, gdrag, rubber, panning, onPointerDown } = useBoardPointer({
    boardRef,
    latest,
    selection,
    mover,
    marquee,
    viewRef,
    setView,
    boardPoint,
    push,
    spaceDown,
  });

  // What a group move carries along while it is dragged or nudged.
  const carry = useMemo(() => {
    const live = gdrag && (gdrag.dx || gdrag.dy) ? gdrag : null;
    if (live) return { ids: within(diagram, live.id), ...live };
    if (!shift || (!shift.dx && !shift.dy)) return null;
    const ids = new Set(
      shift.set.groups.flatMap((id) => [...within(diagram, id)]),
    );
    return { ids, dx: shift.dx, dy: shift.dy };
  }, [diagram, gdrag, shift]);
  const shownPos = useMemo(() => {
    const out = new Map(pos);
    if (drag && out.has(drag.id)) out.set(drag.id, { x: drag.x, y: drag.y });
    if (carry)
      for (const id of carry.ids) {
        const at = out.get(id);
        if (at) out.set(id, { x: at.x + carry.dx, y: at.y + carry.dy });
      }
    if (shift)
      for (const id of mover.moving) {
        const at = out.get(id);
        if (at) out.set(id, { x: at.x + shift.dx, y: at.y + shift.dy });
      }
    return out;
  }, [pos, drag, carry, shift, mover.moving]);
  const groups = useMemo(() => {
    const out = groupRects(diagram);
    const move = (id: string, dx: number, dy: number) => {
      const r = out.get(id);
      if (r) out.set(id, { ...r, x: r.x + dx, y: r.y + dy });
    };
    if (carry) for (const id of carry.ids) move(id, carry.dx, carry.dy);
    if (gdrag) out.set(gdrag.id, gdrag.rect);
    else if (shift)
      for (const id of shift.set.groups) move(id, shift.dx, shift.dy);
    return out;
  }, [diagram, carry, gdrag, shift]);
  // The boundary a dragged component would land in.
  const target = useMemo(() => {
    if (!drag || !diagram.nodes.some((n) => n.id === drag.id)) return "";
    return dropGroup(diagram, drag.id, drag);
  }, [diagram, drag]);
  const noteAt = (id: string, x: number, y: number): Point => {
    if (drag?.id === id) return { x: drag.x, y: drag.y };
    if (shift?.set.notes.includes(id))
      return { x: x + shift.dx, y: y + shift.dy };
    return carry?.ids.has(id) ? { x: x + carry.dx, y: y + carry.dy } : { x, y };
  };
  props.onRemapRef.current = (remap) => {
    const to = (id: string) => remap.get(id) ?? id;
    selection.remap(to);
    setRenaming(to);
    setFresh(to);
  };

  const rectOf = (id: string): Rect | null => {
    const at = shownPos.get(id);
    const node = diagram.nodes.find((n) => n.id === id);
    return at && node ? { x: at.x, y: at.y, ...nodeSize(node) } : null;
  };
  const moving = useMemo(
    () =>
      new Set([...mover.moving, ...(drag ? [drag.id] : (carry?.ids ?? []))]),
    [drag, carry, mover.moving],
  );
  const edges = useRoutes(diagram, diagram, pos, shownPos, groups, moving);

  // A new boundary or note may land outside the visible board.
  useEffect(() => {
    if (!reveal.current) return;
    reveal.current = false;
    fit();
  }, [diagram, fit]);

  const onDoubleClick = (event: React.MouseEvent<HTMLDivElement>) => {
    const t = event.target as HTMLElement;
    const node = t.closest<HTMLElement>("[data-node]");
    const group = t.closest<HTMLElement>("[data-group]");
    if (node && t.closest("[data-title]")) setRenaming(node.dataset.node!);
    else if (group) setRenaming(group.dataset.group!);
    else if (!node && !t.closest("[data-note], [data-edge], [data-resize]"))
      fit();
  };

  const remove = (picks: Picked[]) => {
    pushAll(deleteOps(picks));
    pick(null);
  };
  const setMany = (
    ids: string[],
    field: "shape" | "icon" | "delta" | "group",
    value: string,
  ) => {
    const ops: DiagramOp[] = [];
    for (const id of ids) ops.push({ op: "set", id, field, value });
    pushAll(ops);
  };
  const onKeyDown = (event: React.KeyboardEvent) => {
    if (typing(event.target)) return;
    const arrow = ARROWS[event.key];
    const button =
      event.target instanceof HTMLElement && event.target.closest("button");
    const plain = !button || button.matches("[data-node], [data-note]");
    if (event.key === " " && plain) {
      event.preventDefault();
      setSpaceDown(true);
    } else if (event.key === "Escape") pick(null);
    else if (
      (event.metaKey || event.ctrlKey) &&
      event.key.toLowerCase() === "a"
    ) {
      event.preventDefault();
      selection.selectAll();
    } else if (
      (event.key === "Delete" || event.key === "Backspace") &&
      sel.length
    ) {
      event.preventDefault();
      remove(sel);
    } else if (arrow && sel.length) {
      event.preventDefault();
      const step = event.shiftKey ? NUDGE_FAR : NUDGE;
      mover.nudge(moveSet(diagram, sel), arrow.x * step, arrow.y * step);
    }
  };
  const { menu, closeMenu, onContextMenu, openAddMenu } = useBoardMenu({
    diagram,
    push,
    boardRef,
    boardPoint,
    reveal,
    sel,
    setPicked: pick,
    setRenaming,
    setFresh,
    remove,
    onComment: props.onComment,
  });
  useEffect(() => {
    if (fresh && one?.id !== fresh) setFresh("");
  }, [fresh, one]);
  useEffect(() => {
    if (
      renaming &&
      !diagram.nodes.some((n) => n.id === renaming) &&
      !diagram.groups.some((g) => g.id === renaming)
    )
      setRenaming("");
  }, [renaming, diagram]);

  const from = rubber && rectOf(rubber.from);
  const dot = {
    "--gx": `${view.tx}px`,
    "--gy": `${view.ty}px`,
    "--gs": `${24 * view.s}px`,
    "--gr": `${view.s}px`,
  } as React.CSSProperties;

  return (
    <div
      className="dg"
      onKeyDown={onKeyDown}
      onKeyUp={(event) => event.key === " " && setSpaceDown(false)}
      onBlur={() => setSpaceDown(false)}
    >
      <Toolbar diagram={diagram} />
      <div className="dg-main">
        <div
          className={`dg-board${panning ? " panning" : spaceDown ? " pan" : ""}`}
          ref={boardRef}
          style={dot}
          tabIndex={0}
          aria-label="Diagram board"
          onPointerDown={onPointerDown}
          onDoubleClick={onDoubleClick}
          onContextMenu={onContextMenu}
        >
          <div
            className="dg-world"
            style={{
              transform: `translate(${view.tx}px, ${view.ty}px) scale(${view.s})`,
            }}
          >
            <BoardSvg
              groups={diagram.groups}
              rects={groups}
              isOn={isOn}
              target={target}
              edges={edges}
              rubber={
                rubber && from
                  ? {
                      from: { x: from.x + from.w / 2, y: from.y + from.h / 2 },
                      to: rubber.to,
                    }
                  : null
              }
            />
            <BoardItems
              diagram={diagram}
              groups={groups}
              edges={edges}
              shownPos={shownPos}
              noteAt={noteAt}
              isOn={isOn}
              renaming={renaming}
              onDone={() => {
                setRenaming("");
                boardRef.current?.focus();
              }}
              push={push}
              onReject={() => pick(null)}
            />
            <HeadsSvg edges={edges} isOn={isOn} />
          </div>
          {marquee.box && (
            <div
              className="dg-marquee"
              style={{
                left: marquee.box.x,
                top: marquee.box.y,
                width: marquee.box.w,
                height: marquee.box.h,
              }}
            />
          )}
          <button
            type="button"
            className="dg-fab"
            aria-label="Add"
            aria-haspopup="menu"
            onClick={openAddMenu}
          >
            <Plus size={14} aria-hidden />
          </button>
        </div>
        {one && (
          <Inspect
            key={`${one.type}:${one.id}:${one.id === fresh}`}
            diagram={diagram}
            picked={one}
            focusText={one.id === fresh}
            onOp={(op) => push(op, op.op !== "set")}
            onDelete={() => remove([one])}
            onComment={props.onComment}
          />
        )}
        {sel.length > 1 && (
          <InspectMany
            diagram={diagram}
            picks={sel}
            onSet={setMany}
            onComment={props.onComment}
            onDelete={() => remove(sel)}
          />
        )}
      </div>
      {menu && (
        <ContextMenu at={menu.at} items={menu.items} onClose={closeMenu} />
      )}
      <div className="dg-statusbar">
        <span className="dg-path" title={props.path}>
          {shownPath(props.path, props.cwd)}
        </span>
        <span className="dg-count">
          {countLabel(diagram.nodes.length, diagram.edges.length)}
        </span>
        <span className="dg-spacer" />
        <StatusText status={props.status} readError={props.readError} />
      </div>
    </div>
  );
}

/** The `.canvas` diagram drawn and edited inside the Preview. */
export function DiagramView(props: Props) {
  const onRemapRef = useRef<(remap: Map<string, string>) => void>(() => {});
  const { display, error, status, push } = useDiagramSave({
    ...props,
    onRemap: (remap) => onRemapRef.current(remap),
  });
  if (!display)
    return (
      <div className="pd-alert pv-alert" role="alert">
        This diagram has an error: {error}
      </div>
    );
  return (
    <Board
      {...props}
      onRemapRef={onRemapRef}
      diagram={display}
      status={status}
      push={push}
    />
  );
}
