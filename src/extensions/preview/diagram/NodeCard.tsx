import { iconFor, toneOf } from "./icons.ts";
import { kindOf, nodeSize } from "./model.ts";
import { RenameInput } from "./Parts.tsx";
import { SHAPES } from "./types.ts";
import type { DiagramNode, Point, Shape, Size } from "./types.ts";

const CYL_RY = 8;

const shapeOf = (node: DiagramNode): Shape =>
  SHAPES.includes(node.shape as Shape) ? (node.shape as Shape) : "box";

function Outline(props: { shape: Shape; w: number; h: number; delta: string }) {
  const { shape, w, h } = props;
  if (shape === "diamond")
    return (
      <svg
        className={`dg-shape${props.delta}`}
        width={w}
        height={h}
        aria-hidden
      >
        <polygon
          points={`${w / 2},0.7 ${w - 0.7},${h / 2} ${w / 2},${h - 0.7} 0.7,${h / 2}`}
        />
      </svg>
    );
  if (shape === "cylinder") {
    const rx = w / 2 - 0.7;
    return (
      <svg
        className={`dg-shape${props.delta}`}
        width={w}
        height={h}
        aria-hidden
      >
        <path
          d={`M0.7,${CYL_RY} A${rx},${CYL_RY - 0.7} 0 0 1 ${w - 0.7},${CYL_RY} V${h - CYL_RY} A${rx},${CYL_RY - 0.7} 0 0 1 0.7,${h - CYL_RY} Z`}
        />
        <path
          className="arc"
          d={`M0.7,${CYL_RY} A${rx},${CYL_RY - 0.7} 0 0 0 ${w - 0.7},${CYL_RY}`}
        />
      </svg>
    );
  }
  if (shape === "person")
    return (
      <svg
        className={`dg-shape dg-person${props.delta}`}
        width={34}
        height={30}
        aria-hidden
      >
        <circle cx={17} cy={8} r={6} />
        <path d="M4.5,29 C4.5,21 10,17.5 17,17.5 S29.5,21 29.5,29 Z" />
      </svg>
    );
  return null;
}

export function NodeCard(props: {
  node: DiagramNode;
  at: Point;
  /** Overrides the size while the block is being resized. */
  size?: Size;
  selected: boolean;
  renaming: boolean;
  onRename: (title: string | null) => void;
  onAccept: () => void;
  onReject: () => void;
}) {
  const { node, at, selected, renaming } = props;
  const shape = shapeOf(node);
  const { w, h } = props.size ?? nodeSize(node);
  const { icon, caption, tag } = kindOf(node);
  const Icon = iconFor(icon);
  const delta = node.delta;
  const proposed = node.status === "proposed";
  const tone = toneOf(icon);
  const outside =
    shape === "diamond" || shape === "circle" || shape === "person";
  const outlined = shape === "diamond" || shape === "cylinder";
  const deltaClass = delta ? ` delta-${delta}` : "";
  const sides = [
    ["top", w / 2, 0],
    ["right", w, h / 2],
    ["bottom", w / 2, h],
    ["left", 0, h / 2],
  ] as const;
  const iconEl = Icon && (
    <Icon
      size={12}
      strokeWidth={1.33}
      className={`dg-icon${tone ? ` k-${tone}` : ""}`}
      aria-hidden
    />
  );
  const title = (
    <span className="dg-title" data-title>
      {node.title}
    </span>
  );
  const tech = node.tech && <span className="dg-tech">{node.tech}</span>;
  const inner =
    shape === "box" || shape === "cylinder" ? (
      <>
        {(Icon || caption || tag) && (
          <span className="dg-kind-row">
            {iconEl}
            {caption && <span className="dg-kind">{caption}</span>}
            {tag && (
              <span className={`dg-tag ${proposed ? "new" : tag}`}>{tag}</span>
            )}
          </span>
        )}
        {title}
        {tech}
      </>
    ) : shape === "pill" ? (
      <>
        {iconEl}
        {title}
      </>
    ) : shape === "person" ? (
      <>
        <Outline shape={shape} w={w} h={h} delta={deltaClass} />
        {title}
        {tech}
      </>
    ) : (
      title
    );
  return (
    <div
      className={`dg-node-wrap${selected ? " selected" : ""}`}
      style={{ left: at.x, top: at.y, width: w, height: h }}
      data-node={node.id}
    >
      <button
        type="button"
        className={`dg-node shape-${shape}${deltaClass}`}
        title={tag}
        aria-label={`${(caption || shape).toLowerCase()} ${node.title}`}
      >
        {outlined && <Outline shape={shape} w={w} h={h} delta={deltaClass} />}
        {delta === "new" && !outlined && shape !== "person" && (
          <svg className="dg-dash" width={w} height={h} aria-hidden>
            <rect
              x={0.65}
              y={0.65}
              width={w - 1.3}
              height={h - 1.3}
              rx={shape === "box" ? 8.4 : (h - 1.3) / 2}
            />
          </svg>
        )}
        {inner}
      </button>
      {outside && tag && (
        <span className={`dg-tag out ${proposed ? "new" : tag}`}>{tag}</span>
      )}
      {node.step !== undefined && <span className="dg-step">{node.step}</span>}
      {renaming && (
        <RenameInput
          label="Block title"
          value={node.title}
          onCommit={(value) => props.onRename(value.trim())}
          onCancel={() => props.onRename(null)}
        />
      )}
      {sides.map(([side, x, y]) => (
        <span
          key={side}
          className="dg-port"
          style={{ left: x, top: y }}
          data-port={node.id}
        />
      ))}
      {selected && (
        <span
          className="dg-resize"
          style={{ left: w - 4, top: h - 4 }}
          data-node-resize={node.id}
          aria-label={`Resize ${node.title}`}
        />
      )}
      {proposed && selected && (
        <div className="dg-bar" style={{ top: h + 6 }}>
          <button type="button" onClick={props.onAccept}>
            Accept
          </button>
          <button type="button" onClick={props.onReject}>
            Reject
          </button>
        </div>
      )}
    </div>
  );
}
