import type { LabelSpot } from "./model.ts";
import { NodeCard } from "./NodeCard.tsx";
import { GroupBox, NotePin } from "./Parts.tsx";
import type {
  Diagram,
  DiagramEdge,
  DiagramGroup,
  DiagramOp,
  Picked,
  Point,
  Rect,
  Size,
} from "./types.ts";
import type { SaveStatus } from "./useDiagramSave.ts";

export type IsOn = (type: Picked["type"], id: string) => boolean;

const arrow = (a: Point, b: Point) => {
  const angle = Math.atan2(b.y - a.y, b.x - a.x);
  const tip = (turn: number) =>
    `${b.x - 8 * Math.cos(angle + turn)},${b.y - 8 * Math.sin(angle + turn)}`;
  return `M${tip(0.5)} L${b.x},${b.y} L${tip(-0.5)}`;
};

export function StatusText(props: { status: SaveStatus; readError?: string }) {
  const { status } = props;
  const problem =
    status.state === "error" ? status.message : props.readError || "";
  if (problem)
    return (
      <span className="dg-status-end warn" role="alert">
        {problem}
      </span>
    );
  return (
    <span className="dg-status-end">
      {status.state === "saving" ? "Saving…" : "Saved"}
    </span>
  );
}

export function BoardSvg(props: {
  groups: DiagramGroup[];
  rects: Map<string, Rect>;
  isOn: IsOn;
  /** The boundary a dragged component would land in. */
  target: string;
  edges: { edge: DiagramEdge; points: Point[] }[];
  rubber: { from: Point; to: Point } | null;
}) {
  const { rubber } = props;
  return (
    <svg className="dg-svg" aria-hidden>
      {props.groups.map((g) => {
        const r = props.rects.get(g.id);
        return (
          r && (
            <rect
              key={g.id}
              className={`dg-group${props.isOn("group", g.id) ? " selected" : ""}${props.target === g.id ? " target" : ""}`}
              x={r.x + 0.5}
              y={r.y + 0.5}
              width={r.w - 1}
              height={r.h - 1}
              rx={12}
            />
          )
        );
      })}
      {props.edges.map(({ edge, points }) => {
        const d = points.map((q, i) => `${i ? "L" : "M"}${q.x},${q.y}`);
        const on = props.isOn("edge", edge.id);
        return (
          <g
            key={edge.id}
            className={`dg-edge${edge.style === "guess" || edge.style === "dashed" ? ` ${edge.style}` : ""}${on ? " selected" : ""}`}
          >
            <path d={d.join(" ")} className="line" />
            <path d={d.join(" ")} className="hit" data-edge={edge.id} />
          </g>
        );
      })}
      {rubber && (
        <path
          className="dg-rubber"
          d={`M${rubber.from.x},${rubber.from.y} L${rubber.to.x},${rubber.to.y}`}
        />
      )}
    </svg>
  );
}

/** Arrow heads, drawn above the blocks so a shape never covers one. */
export function HeadsSvg(props: {
  edges: { edge: DiagramEdge; points: Point[] }[];
  isOn: IsOn;
}) {
  return (
    <svg className="dg-svg dg-heads" aria-hidden>
      {props.edges.map(({ edge, points }) => {
        const n = points.length;
        const cls = `dg-edge${edge.style === "guess" ? " guess" : ""}${props.isOn("edge", edge.id) ? " selected" : ""}`;
        return (
          <g key={edge.id} className={cls}>
            {edge.arrow !== "none" && edge.arrow !== "start" && (
              <path d={arrow(points[n - 2], points[n - 1])} className="head" />
            )}
            {(edge.arrow === "both" || edge.arrow === "start") && (
              <path d={arrow(points[1], points[0])} className="head" />
            )}
          </g>
        );
      })}
    </svg>
  );
}

export function BoardItems(props: {
  diagram: Diagram;
  groups: Map<string, Rect>;
  edges: {
    edge: DiagramEdge;
    label: LabelSpot;
  }[];
  shownPos: Map<string, Point>;
  /** Live sizes of blocks being resized. */
  sizes: Map<string, Size>;
  noteAt: (id: string, x: number, y: number) => Point;
  isOn: IsOn;
  renaming: string;
  onDone: () => void;
  push: (op: DiagramOp) => void;
  onReject: () => void;
}) {
  const { diagram, isOn: on, renaming, push } = props;
  return (
    <>
      {diagram.groups.map((g) => {
        const r = props.groups.get(g.id);
        return (
          r && (
            <GroupBox
              key={g.id}
              group={g}
              rect={r}
              selected={on("group", g.id)}
              renaming={renaming === g.id}
              onRename={(label) => {
                props.onDone();
                if (label && label !== g.label)
                  push({ op: "set", id: g.id, field: "label", value: label });
              }}
            />
          )
        );
      })}
      {diagram.nodes.map((node) => {
        const at = props.shownPos.get(node.id);
        return (
          at && (
            <NodeCard
              key={node.id}
              node={node}
              at={at}
              size={props.sizes.get(node.id)}
              selected={on("node", node.id)}
              renaming={renaming === node.id}
              onRename={(title) => {
                props.onDone();
                if (title && title !== node.title)
                  push({
                    op: "set",
                    id: node.id,
                    field: "title",
                    value: title,
                  });
              }}
              onAccept={() => push({ op: "accept", id: node.id })}
              onReject={() => {
                push({ op: "reject", id: node.id });
                props.onReject();
              }}
            />
          )
        );
      })}
      {/* Above the blocks, so a block never hides a label. */}
      {props.edges.map(
        ({ edge, label }) =>
          edge.label && (
            <span
              key={edge.id}
              className={`dg-label${label.vertical ? " vertical" : ""}${label.flip ? " flip" : ""}${edge.style === "guess" ? " guess" : ""}`}
              style={{ left: label.x, top: label.y }}
            >
              {edge.label}
            </span>
          ),
      )}
      {diagram.notes.map((note) => (
        <NotePin
          key={note.id}
          note={note}
          at={props.noteAt(note.id, note.x, note.y)}
          selected={on("note", note.id)}
        />
      ))}
    </>
  );
}
