// The `.arch.json` diagram file: blocks, links, groups and notes. One
// universal block covers an app architecture, an agent's or a skill's order of
// steps, a module as-is and to-be: its look comes from optional properties
// (shape, icon, caption, step, delta), not from a type. The file is the only
// source of truth; the agent and the owner both edit it. Unknown fields are
// kept on every save.

export const DIAGRAM_VERSION = 1;

/** A `box` block is drawn at this size (Figma `Canvas/Component`); other
 * shapes have their own size, see `nodeSize` in model.ts. */
export const NODE_W = 170;
export const NODE_H = 62;

/** Presets, not types: a preset only supplies a default icon and caption. */
export type NodeKind = "app" | "service" | "data" | "external";
export type Delta = "new" | "changed" | "exists" | "removed";
export type Shape =
  "box" | "pill" | "diamond" | "cylinder" | "person" | "circle";

export const SHAPES: readonly Shape[] = [
  "box",
  "pill",
  "diamond",
  "cylinder",
  "person",
  "circle",
];

/** Icon names a block or a group may use (drawn with lucide icons). */
export const ICONS = [
  "laptop",
  "server",
  "database",
  "cloud",
  "phone",
  "globe",
  "bot",
  "user",
  "users",
  "terminal",
  "code",
  "puzzle",
  "file",
  "folder",
  "message",
  "key",
  "lock",
  "zap",
  "play",
  "flag",
  "check",
  "git",
  "cpu",
  "box",
] as const;
export type IconName = (typeof ICONS)[number];

export type DiagramNode = {
  id: string;
  /** Legacy preset; new blocks use `icon` and `caption` instead. */
  kind?: NodeKind;
  title: string;
  /** Default `box`. */
  shape?: Shape;
  icon?: IconName;
  /** Small uppercase line above the title ("SERVICE", "SKILL", "STEP"). */
  caption?: string;
  /** One line under the title: technology, transport, a short role. */
  tech?: string;
  /** Order number badge for steps (1, 2, 3...). */
  step?: number;
  /** As-is / to-be mark. Absent = no mark. */
  delta?: Delta;
  /** Id of the boundary the component sits in. */
  group?: string;
  /** Top-left on the board. Absent = placed by the Preview. */
  x?: number;
  y?: number;
  /** Set by the agent; the owner accepts (removes it) or rejects (deletes). */
  status?: "proposed";
  [extra: string]: unknown;
};

export type DiagramEdge = {
  id: string;
  from: string;
  to: string;
  label?: string;
  /** `dashed` is a neutral dashed line; `guess` an unconfirmed link (dashed
   * warning line). Default `plain`. */
  style?: "plain" | "dashed" | "guess";
  /** Arrow heads. Default `end`. */
  arrow?: "end" | "both" | "none";
  status?: "proposed";
  [extra: string]: unknown;
};

/** A group: a trust boundary (Phone, Cloud...), a lane (Agent, Owner, CI) or any
 * frame. Without bounds it wraps its members; with bounds it still grows to
 * contain them. */
export type DiagramGroup = {
  id: string;
  label: string;
  icon?: IconName;
  x?: number;
  y?: number;
  w?: number;
  h?: number;
  [extra: string]: unknown;
};

/** A question or warning pinned on the board (Figma `Canvas/Pin`). */
export type DiagramNote = {
  id: string;
  text: string;
  x: number;
  y: number;
  [extra: string]: unknown;
};

export type Diagram = {
  version: 1;
  title?: string;
  groups: DiagramGroup[];
  nodes: DiagramNode[];
  edges: DiagramEdge[];
  notes: DiagramNote[];
  [extra: string]: unknown;
};

/** One owner edit. Edits are kept as ops, not as a whole file, so a save can
 * replay them on top of whatever the agent wrote meanwhile. */
export type DiagramOp =
  | { op: "add-node"; node: DiagramNode }
  | { op: "add-group"; group: DiagramGroup }
  | { op: "add-note"; note: DiagramNote }
  /** Sets the boundary to `rect` and shifts its placed members by dx/dy. */
  | { op: "move-group"; id: string; rect: Rect; dx: number; dy: number }
  | { op: "resize-group"; id: string; rect: Rect }
  | { op: "connect"; edge: DiagramEdge }
  | { op: "move"; id: string; x: number; y: number }
  | {
      op: "set";
      id: string;
      field:
        | "title"
        | "shape"
        | "caption"
        | "step"
        | "arrow"
        | "delta"
        | "tech"
        | "group"
        | "label"
        | "style"
        | "icon"
        | "text";
      /** Empty string removes an optional field. */
      value: string;
    }
  | { op: "delete"; id: string }
  | { op: "accept"; id: string }
  | { op: "reject"; id: string };

export type Rect = { x: number; y: number; w: number; h: number };
export type Point = { x: number; y: number };

/** One selected thing on the board. */
export type Picked = {
  type: "node" | "edge" | "note" | "group";
  id: string;
};
