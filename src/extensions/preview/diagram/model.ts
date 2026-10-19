// Pure model of the `.arch.json` diagram: parse, edit by ops, place, route.
// No React, no bridge.

import {
  ICONS as ICON_NAMES,
  NODE_H,
  NODE_W,
  SHAPES as SHAPE_NAMES,
  type Diagram,
  type DiagramEdge,
  type DiagramGroup,
  type DiagramNode,
  type DiagramOp,
  type IconName,
  type Point,
  type Rect,
} from "./types.ts";

const KINDS: readonly string[] = ["app", "service", "data", "external"];
const DELTAS: readonly string[] = ["new", "changed", "exists", "removed"];
const STYLES: readonly string[] = ["plain", "dashed", "guess"];
const ARROWS: readonly string[] = ["end", "both", "none"];
const ICONS: readonly string[] = ICON_NAMES;
const SHAPES: readonly string[] = SHAPE_NAMES;
const MAX_ICON = 40;
const MAX_STEP = 999;

const SIZES: Record<string, { w: number; h: number }> = {
  box: { w: NODE_W, h: NODE_H },
  pill: { w: 170, h: 44 },
  diamond: { w: 150, h: 90 },
  cylinder: { w: 170, h: 72 },
  person: { w: 120, h: 84 },
  circle: { w: 72, h: 72 },
};

/** The drawn size of a block, from its shape (default box). */
export function nodeSize(node: { shape?: unknown }): { w: number; h: number } {
  return SIZES[node.shape as string] ?? SIZES.box;
}

const KIND_PRESETS: Record<string, { icon: IconName; caption: string }> = {
  app: { icon: "laptop", caption: "App" },
  service: { icon: "server", caption: "Service" },
  data: { icon: "database", caption: "Data" },
  external: { icon: "cloud", caption: "External" },
};

/** Icon and caption a block shows: its own win, else the legacy kind's. */
export function presetOf(node: DiagramNode): {
  icon?: IconName;
  caption?: string;
} {
  const preset = node.kind ? KIND_PRESETS[node.kind] : undefined;
  const out: { icon?: IconName; caption?: string } = {};
  const icon = knownIcon(node.icon) ? node.icon : preset?.icon;
  const caption = node.caption ?? preset?.caption;
  if (icon !== undefined) out.icon = icon;
  if (caption !== undefined) out.caption = caption;
  return out;
}

/** The Add-menu presets: a starting look for a new block, not a type. */
export const PRESETS: readonly {
  id: string;
  label: string;
  node: Partial<DiagramNode>;
}[] = [
  { id: "step", label: "Step", node: { shape: "box", caption: "Step" } },
  { id: "start", label: "Start / end", node: { shape: "pill" } },
  { id: "decision", label: "Decision", node: { shape: "diamond" } },
  { id: "app", label: "App", node: { icon: "laptop", caption: "App" } },
  {
    id: "service",
    label: "Service",
    node: { icon: "server", caption: "Service" },
  },
  {
    id: "data",
    label: "Data store",
    node: { shape: "cylinder", icon: "database", caption: "Data" },
  },
  {
    id: "external",
    label: "External",
    node: { icon: "cloud", caption: "External" },
  },
  { id: "agent", label: "Agent", node: { icon: "bot", caption: "Agent" } },
  { id: "skill", label: "Skill", node: { icon: "puzzle", caption: "Skill" } },
  { id: "person", label: "Person", node: { shape: "person", icon: "user" } },
];

const validStep = (v: unknown): v is number =>
  typeof v === "number" && Number.isInteger(v) && v >= 1 && v <= MAX_STEP;
// An icon name the table does not know is not an error: it draws no icon.
const validIconName = (v: unknown): v is string =>
  typeof v === "string" && v.length <= MAX_ICON;
const knownIcon = (v: unknown): v is IconName => ICONS.includes(v as string);

const GAP = 20;
const STEP_X = 210;
const STEP_Y = 90;
const LATTICE_X = 26;
const LATTICE_Y = 46;
const LATTICE_COLS = 3;
// Wrap padding matches the tightest boundary in Figma frame D; new nodes are
// placed further in so a slot never touches the boundary line.
const GROUP_PAD_X = 10;
const GROUP_PAD_TOP = 30;
const GROUP_PAD_BOTTOM = 8;
const SLOT_INSET = 20;
const MAX_SLOTS = 100000;
const MAX_ITEMS = 1000;
export const MAX_COORD = 100000;
const MAX_CHECKS = 2_000_000;
const MAX_CELLS = 4096;
const CELL = 256;

type Obj = Record<string, unknown>;

const isObj = (v: unknown): v is Obj =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const num = (v: unknown): v is number =>
  typeof v === "number" && Number.isFinite(v);
const hasPos = (n: { x?: unknown; y?: unknown }) => num(n.x) && num(n.y);
export const MIN_GROUP_W = 190;
export const MIN_GROUP_H = 100;

const badNum = (o: Obj, fields: string[]) =>
  fields.find((f) => {
    const v = o[f];
    if (v === undefined) return false;
    if (!num(v)) return true;
    return f === "w" || f === "h"
      ? v <= 0 || v > MAX_COORD
      : Math.abs(v) > MAX_COORD;
  });

function allIds(d: Diagram): Set<string> {
  const ids = new Set<string>();
  for (const list of [d.groups, d.nodes, d.edges, d.notes]) {
    for (const item of list) ids.add(item.id);
  }
  return ids;
}

export function parseDiagram(
  text: string,
): { diagram: Diagram } | { error: string } {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch (e) {
    return { error: `not valid JSON: ${(e as Error).message}` };
  }
  if (!isObj(raw)) return { error: "the file must be a JSON object" };
  if (raw.version !== 1) {
    return { error: `unsupported version ${JSON.stringify(raw.version)}` };
  }
  const lists: Record<string, Obj[]> = {};
  for (const key of ["groups", "nodes", "edges", "notes"]) {
    const value = raw[key] ?? [];
    if (!Array.isArray(value)) return { error: `${key} must be an array` };
    lists[key] = value as Obj[];
  }
  const total = Object.values(lists).reduce((n, l) => n + l.length, 0);
  if (total > MAX_ITEMS) {
    return { error: `too many items (${total}); the limit is ${MAX_ITEMS}` };
  }
  const ids = new Set<string>();
  const kindOfId = new Map<string, string>();
  for (const key of ["groups", "nodes", "edges", "notes"]) {
    for (let i = 0; i < lists[key].length; i++) {
      const item = lists[key][i];
      if (!isObj(item)) return { error: `${key}[${i}]: must be an object` };
      if (typeof item.id !== "string" || item.id === "") {
        return { error: `${key}[${i}]: id must be a non-empty string` };
      }
      if (ids.has(item.id)) {
        return { error: `${key}[${i}] (${item.id}): duplicate id` };
      }
      ids.add(item.id);
      kindOfId.set(item.id, key);
    }
  }
  const fail = (key: string, i: number, msg: string) => ({
    error: `${key}[${i}] (${lists[key][i].id}): ${msg}`,
  });
  for (let i = 0; i < lists.groups.length; i++) {
    const g = lists.groups[i];
    if (typeof g.label !== "string")
      return fail("groups", i, "label must be a string");
    if (g.icon !== undefined && !validIconName(g.icon)) {
      return fail("groups", i, "icon must be a string");
    }
    const bad = badNum(g, ["x", "y", "w", "h"]);
    if (bad) return fail("groups", i, `${bad} must be a finite number`);
  }
  for (let i = 0; i < lists.nodes.length; i++) {
    const n = lists.nodes[i];
    if (n.kind !== undefined && !KINDS.includes(n.kind as string)) {
      return fail("nodes", i, `unknown kind ${JSON.stringify(n.kind)}`);
    }
    if (typeof n.title !== "string")
      return fail("nodes", i, "title must be a string");
    if (n.shape !== undefined && !SHAPES.includes(n.shape as string)) {
      return fail("nodes", i, `unknown shape ${JSON.stringify(n.shape)}`);
    }
    if (n.icon !== undefined && !validIconName(n.icon)) {
      return fail("nodes", i, "icon must be a string");
    }
    for (const f of ["caption", "tech"]) {
      if (n[f] !== undefined && typeof n[f] !== "string") {
        return fail("nodes", i, `${f} must be a string`);
      }
    }
    if (n.step !== undefined && !validStep(n.step)) {
      return fail("nodes", i, `step must be an integer from 1 to ${MAX_STEP}`);
    }
    if (n.delta !== undefined && !DELTAS.includes(n.delta as string)) {
      return fail("nodes", i, `unknown delta ${JSON.stringify(n.delta)}`);
    }
    if (n.status !== undefined && n.status !== "proposed") {
      return fail("nodes", i, `unknown status ${JSON.stringify(n.status)}`);
    }
    const bad = badNum(n, ["x", "y"]);
    if (bad) return fail("nodes", i, `${bad} must be a finite number`);
    if (n.group !== undefined) {
      if (typeof n.group !== "string" || kindOfId.get(n.group) !== "groups") {
        return fail("nodes", i, `unknown group ${JSON.stringify(n.group)}`);
      }
    }
  }
  for (let i = 0; i < lists.edges.length; i++) {
    const e = lists.edges[i];
    for (const end of ["from", "to"]) {
      const target = e[end];
      if (typeof target !== "string" || kindOfId.get(target) !== "nodes") {
        return fail(
          "edges",
          i,
          `${end} references unknown node ${JSON.stringify(target)}`,
        );
      }
    }
    if (e.label !== undefined && typeof e.label !== "string") {
      return fail("edges", i, "label must be a string");
    }
    if (e.style !== undefined && !STYLES.includes(e.style as string)) {
      return fail("edges", i, `unknown style ${JSON.stringify(e.style)}`);
    }
    if (e.arrow !== undefined && !ARROWS.includes(e.arrow as string)) {
      return fail("edges", i, `unknown arrow ${JSON.stringify(e.arrow)}`);
    }
    if (e.status !== undefined && e.status !== "proposed") {
      return fail("edges", i, `unknown status ${JSON.stringify(e.status)}`);
    }
  }
  for (let i = 0; i < lists.notes.length; i++) {
    const n = lists.notes[i];
    if (typeof n.text !== "string")
      return fail("notes", i, "text must be a string");
    if (!num(n.x) || !num(n.y) || badNum(n, ["x", "y"]))
      return fail("notes", i, "x and y must be finite numbers in range");
  }
  if (raw.title !== undefined && typeof raw.title !== "string") {
    return { error: "title must be a string" };
  }
  return {
    diagram: { ...raw, ...lists, version: 1 } as unknown as Diagram,
  };
}

export function serializeDiagram(d: Diagram): string {
  return `${JSON.stringify(d, null, 2)}\n`;
}

function without<T extends Obj>(o: T, ...keys: string[]): T {
  const copy: Obj = { ...o };
  for (const k of keys) delete copy[k];
  return copy as T;
}

export function applyOp(d: Diagram, op: DiagramOp): Diagram {
  switch (op.op) {
    case "add-node": {
      if (allIds(d).has(op.node.id)) return d;
      if (badNum(op.node as Obj, ["x", "y"])) return d;
      return { ...d, nodes: [...d.nodes, op.node] };
    }
    case "add-group": {
      const g = op.group as Obj;
      const ok =
        isObj(g) &&
        typeof g.id === "string" &&
        g.id !== "" &&
        typeof g.label === "string" &&
        (g.icon === undefined || ICONS.includes(g.icon as string)) &&
        !badNum(g, ["x", "y", "w", "h"]);
      if (!ok || allIds(d).has(op.group.id)) return d;
      return { ...d, groups: [...d.groups, op.group] };
    }
    case "add-note": {
      const n = op.note as Obj;
      const ok =
        isObj(n) &&
        typeof n.id === "string" &&
        n.id !== "" &&
        typeof n.text === "string" &&
        num(n.x) &&
        num(n.y) &&
        !badNum(n, ["x", "y"]);
      if (!ok || allIds(d).has(op.note.id)) return d;
      return { ...d, notes: [...d.notes, op.note] };
    }
    case "move-group": {
      const r = op.rect as Obj;
      if (!d.groups.some((g) => g.id === op.id)) return d;
      if (!isObj(r) || !num(op.dx) || !num(op.dy)) return d;
      if (!num(r.x) || !num(r.y) || !num(r.w) || !num(r.h)) return d;
      if (badNum(r, ["x", "y", "w", "h"])) return d;
      const shifted = d.nodes.some(
        (n) =>
          n.group === op.id &&
          hasPos(n) &&
          (Math.abs((n.x as number) + op.dx) > MAX_COORD ||
            Math.abs((n.y as number) + op.dy) > MAX_COORD),
      );
      if (shifted) return d;
      return {
        ...d,
        groups: d.groups.map((g) =>
          g.id === op.id ? { ...g, ...op.rect } : g,
        ),
        nodes: d.nodes.map((n) =>
          n.group === op.id && hasPos(n)
            ? { ...n, x: (n.x as number) + op.dx, y: (n.y as number) + op.dy }
            : n,
        ),
      };
    }
    case "resize-group": {
      const r = op.rect as Obj;
      if (!d.groups.some((g) => g.id === op.id)) return d;
      if (!isObj(r) || !num(r.x) || !num(r.y) || !num(r.w) || !num(r.h)) {
        return d;
      }
      if (badNum(r, ["x", "y", "w", "h"])) return d;
      const rect = {
        x: op.rect.x,
        y: op.rect.y,
        w: Math.max(MIN_GROUP_W, op.rect.w),
        h: Math.max(MIN_GROUP_H, op.rect.h),
      };
      return {
        ...d,
        groups: d.groups.map((g) => (g.id === op.id ? { ...g, ...rect } : g)),
      };
    }
    case "connect": {
      const e = op.edge;
      const has = (id: string) => d.nodes.some((n) => n.id === id);
      if (!has(e.from) || !has(e.to) || e.from === e.to) return d;
      if (allIds(d).has(e.id)) return d;
      if (d.edges.some((x) => x.from === e.from && x.to === e.to)) return d;
      return { ...d, edges: [...d.edges, e] };
    }
    case "move": {
      if (!num(op.x) || !num(op.y)) return d;
      const place = <T extends { id: string }>(list: T[]) =>
        list.map((i) => (i.id === op.id ? { ...i, x: op.x, y: op.y } : i));
      if (d.nodes.some((n) => n.id === op.id)) {
        return { ...d, nodes: place(d.nodes) };
      }
      if (d.notes.some((n) => n.id === op.id)) {
        return { ...d, notes: place(d.notes) };
      }
      return d;
    }
    case "set":
      return applySet(d, op.id, op.field, op.value);
    case "delete":
    case "reject":
      return remove(d, op.id);
    case "accept": {
      const strip = <T extends { id: string }>(list: T[]) =>
        list.map((i) => (i.id === op.id ? without(i, "status") : i));
      return { ...d, nodes: strip(d.nodes), edges: strip(d.edges) };
    }
  }
}

function remove(d: Diagram, id: string): Diagram {
  if (d.nodes.some((n) => n.id === id)) {
    return {
      ...d,
      nodes: d.nodes.filter((n) => n.id !== id),
      edges: d.edges.filter((e) => e.from !== id && e.to !== id),
    };
  }
  if (d.edges.some((e) => e.id === id)) {
    return { ...d, edges: d.edges.filter((e) => e.id !== id) };
  }
  if (d.notes.some((n) => n.id === id)) {
    return { ...d, notes: d.notes.filter((n) => n.id !== id) };
  }
  if (d.groups.some((g) => g.id === id)) {
    return {
      ...d,
      groups: d.groups.filter((g) => g.id !== id),
      nodes: d.nodes.map((n) => (n.group === id ? without(n, "group") : n)),
    };
  }
  return d;
}

function applySet(
  d: Diagram,
  id: string,
  field: string,
  value: string,
): Diagram {
  const node = d.nodes.find((n) => n.id === id);
  const edge = d.edges.find((e) => e.id === id);
  const group = d.groups.find((g) => g.id === id);
  const clear = value === "";
  const patch = <T extends { id: string }>(
    list: T[],
    next: (item: T) => T,
  ): T[] => list.map((i) => (i.id === id ? next(i) : i));
  const setOrClear = <T extends Obj>(item: T): T =>
    clear ? without(item, field) : { ...item, [field]: value };

  if (node) {
    const update = (next: (n: DiagramNode) => DiagramNode) => ({
      ...d,
      nodes: patch(d.nodes, next),
    });
    switch (field) {
      case "title":
        return clear ? d : update((n) => ({ ...n, title: value }));
      case "shape":
        return clear || SHAPES.includes(value)
          ? update((n) => setOrClear(n) as DiagramNode)
          : d;
      case "icon":
        return clear || ICONS.includes(value)
          ? update((n) => setOrClear(n) as DiagramNode)
          : d;
      case "step": {
        if (clear) return update((n) => without(n, "step"));
        const step = Number(value);
        return /^\d+$/.test(value) && validStep(step)
          ? update((n) => ({ ...n, step }))
          : d;
      }
      case "delta":
        return clear || DELTAS.includes(value)
          ? update((n) => setOrClear(n) as DiagramNode)
          : d;
      case "caption":
      case "tech":
        return update((n) => setOrClear(n) as DiagramNode);
      case "group":
        return clear || d.groups.some((g) => g.id === value)
          ? update((n) => setOrClear(n) as DiagramNode)
          : d;
      default:
        return d;
    }
  }
  if (edge) {
    if (field === "label") {
      return {
        ...d,
        edges: patch(d.edges, (e) => setOrClear(e) as DiagramEdge),
      };
    }
    const allowed =
      field === "style" ? STYLES : field === "arrow" ? ARROWS : null;
    if (allowed && (clear || allowed.includes(value))) {
      return {
        ...d,
        edges: patch(d.edges, (e) => setOrClear(e) as DiagramEdge),
      };
    }
    return d;
  }
  if (group) {
    if (field === "label" && !clear) {
      return {
        ...d,
        groups: patch(d.groups, (g) => ({ ...g, label: value })),
      };
    }
    if (field === "icon" && (clear || ICONS.includes(value))) {
      return {
        ...d,
        groups: patch(d.groups, (g) => setOrClear(g) as DiagramGroup),
      };
    }
    return d;
  }
  const note = d.notes.find((n) => n.id === id);
  if (note && field === "text" && !clear) {
    return { ...d, notes: patch(d.notes, (n) => ({ ...n, text: value })) };
  }
  return d;
}

export function applyOps(d: Diagram, ops: DiagramOp[]): Diagram {
  return ops.reduce(applyOp, d);
}

export function newId(d: Diagram, base: string): string {
  const slug =
    base
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "item";
  const ids = allIds(d);
  if (!ids.has(slug)) return slug;
  for (let n = 2; ; n++) {
    if (!ids.has(`${slug}-${n}`)) return `${slug}-${n}`;
  }
}

function overlaps(a: Rect, b: Rect, gap: number): boolean {
  return (
    a.x < b.x + b.w + gap &&
    b.x < a.x + a.w + gap &&
    a.y < b.y + b.h + gap &&
    b.y < a.y + a.h + gap
  );
}

function bbox(rects: Rect[]): Rect {
  const x = Math.min(...rects.map((r) => r.x));
  const y = Math.min(...rects.map((r) => r.y));
  const right = Math.max(...rects.map((r) => r.x + r.w));
  const bottom = Math.max(...rects.map((r) => r.y + r.h));
  return { x, y, w: right - x, h: bottom - y };
}

const nodeRect = (p: Point, n: DiagramNode): Rect => ({
  x: p.x,
  y: p.y,
  ...nodeSize(n),
});

export function groupRects(
  d: Diagram,
  pos: Map<string, Point>,
): Map<string, Rect> {
  const out = new Map<string, Rect>();
  for (const g of d.groups) {
    const parts: Rect[] = [];
    if (num(g.x) && num(g.y) && num(g.w) && num(g.h)) {
      parts.push({ x: g.x, y: g.y, w: g.w, h: g.h });
    }
    const members = d.nodes
      .filter((n) => n.group === g.id && pos.has(n.id))
      .map((n) => nodeRect(pos.get(n.id) as Point, n));
    if (members.length > 0) {
      const b = bbox(members);
      parts.push({
        x: b.x - GROUP_PAD_X,
        y: b.y - GROUP_PAD_TOP,
        w: b.w + 2 * GROUP_PAD_X,
        h: b.h + GROUP_PAD_TOP + GROUP_PAD_BOTTOM,
      });
    }
    if (parts.length > 0) out.set(g.id, bbox(parts));
  }
  return out;
}

/** Rects in grid buckets, so a check reads only the rects nearby. A rect that
 * would fill too many cells (huge or far away) goes to a short linear list. */
function rectIndex() {
  const cells = new Map<string, Rect[]>();
  const big: Rect[] = [];
  const range = (r: Rect, pad: number) => {
    const x0 = Math.floor((r.x - pad) / CELL);
    const y0 = Math.floor((r.y - pad) / CELL);
    const x1 = Math.floor((r.x + r.w + pad) / CELL);
    const y1 = Math.floor((r.y + r.h + pad) / CELL);
    // Cell indexes past 2^31 could not be walked by `++` (or are not finite).
    const far = ![x0, y0, x1, y1].every((v) => Math.abs(v) < 2 ** 31);
    return {
      x0,
      y0,
      x1,
      y1,
      count: far ? Infinity : (x1 - x0 + 1) * (y1 - y0 + 1),
    };
  };
  return {
    add(r: Rect) {
      const c = range(r, 0);
      if (!(c.count <= MAX_CELLS)) {
        big.push(r);
        return;
      }
      for (let cx = c.x0; cx <= c.x1; cx++) {
        for (let cy = c.y0; cy <= c.y1; cy++) {
          const key = `${cx},${cy}`;
          const list = cells.get(key);
          if (list) list.push(r);
          else cells.set(key, [r]);
        }
      }
    },
    hit(r: Rect, pad: number): boolean {
      if (big.some((q) => overlaps(r, q, pad))) return true;
      const c = range(r, pad);
      if (!(c.count <= MAX_CELLS)) return c.count !== Infinity;
      for (let cx = c.x0; cx <= c.x1; cx++) {
        for (let cy = c.y0; cy <= c.y1; cy++) {
          if (cells.get(`${cx},${cy}`)?.some((q) => overlaps(r, q, pad))) {
            return true;
          }
        }
      }
      return false;
    },
  };
}

export function placeNodes(d: Diagram): Map<string, Point> {
  const pos = new Map<string, Point>();
  for (const n of d.nodes) {
    if (hasPos(n)) pos.set(n.id, { x: n.x as number, y: n.y as number });
  }
  const nodeIdx = rectIndex();
  for (const n of d.nodes) {
    const pt = pos.get(n.id);
    if (pt) nodeIdx.add(nodeRect(pt, n));
  }
  // Group rects are built once and only grow as members are placed.
  const groupIdx = rectIndex();
  const grects = groupRects(d, pos);
  for (const r of grects.values()) groupIdx.add(r);

  let checks = 0;
  // Slots found taken stay taken, so a lattice scan resumes where it stopped.
  const cursors = new Map<string, number>();
  const scan = (
    cols: number,
    x0: number,
    y0: number,
    useGroups: boolean,
    node: DiagramNode,
  ): Point | null => {
    const { w, h } = nodeSize(node);
    const key = `${cols},${x0},${y0},${useGroups},${w},${h}`;
    for (let k = cursors.get(key) ?? 0; k < MAX_SLOTS; k++) {
      if (++checks > MAX_CHECKS) return null;
      const p = {
        x: x0 + (k % cols) * STEP_X,
        y: y0 + Math.floor(k / cols) * STEP_Y,
      };
      const r = { ...p, w, h };
      if (nodeIdx.hit(r, GAP) || (useGroups && groupIdx.hit(r, 0))) continue;
      cursors.set(key, k);
      return p;
    }
    return null;
  };

  for (const n of d.nodes) {
    if (pos.has(n.id)) continue;
    const g = n.group ? d.groups.find((x) => x.id === n.group) : undefined;
    const { w: nw } = nodeSize(n);
    let p: Point | null = null;
    if (g && num(g.x) && num(g.y) && num(g.w) && num(g.h)) {
      const cols = Math.max(
        1,
        Math.floor((g.w - 2 * SLOT_INSET - nw) / STEP_X) + 1,
      );
      p = scan(cols, g.x + SLOT_INSET, g.y + GROUP_PAD_TOP, false, n);
    } else if (g) {
      const members = d.nodes
        .filter((m) => m.group === g.id && pos.has(m.id))
        .map((m) => nodeRect(pos.get(m.id) as Point, m));
      if (members.length > 0) {
        const b = bbox(members);
        const cols = Math.floor((b.w - nw) / STEP_X) + 2;
        p = scan(Math.max(2, cols), b.x, b.y, false, n);
      }
    }
    p ??= scan(LATTICE_COLS, LATTICE_X, LATTICE_Y, true, n);
    const at = p ?? { x: LATTICE_X, y: LATTICE_Y };
    pos.set(n.id, at);
    const r = nodeRect(at, n);
    nodeIdx.add(r);
    if (g) {
      const padded = {
        x: r.x - GROUP_PAD_X,
        y: r.y - GROUP_PAD_TOP,
        w: r.w + 2 * GROUP_PAD_X,
        h: r.h + GROUP_PAD_TOP + GROUP_PAD_BOTTOM,
      };
      const grown = bbox([grects.get(g.id) ?? padded, padded]);
      grects.set(g.id, grown);
      groupIdx.add(grown);
    }
  }
  return pos;
}

export function materialize(d: Diagram, pos: Map<string, Point>): Diagram {
  return {
    ...d,
    nodes: d.nodes.map((n) => {
      const p = pos.get(n.id);
      return p && !hasPos(n) ? { ...n, x: p.x, y: p.y } : n;
    }),
  };
}

function overlapRange(
  a0: number,
  a1: number,
  b0: number,
  b1: number,
): [number, number] | null {
  const lo = Math.max(a0, b0);
  const hi = Math.min(a1, b1);
  return hi > lo ? [lo, hi] : null;
}

/** Where a straight segment between two overlapping ranges runs. */
function crossPos(
  a0: number,
  a1: number,
  b0: number,
  b1: number,
  [lo, hi]: [number, number],
): number {
  const ac = (a0 + a1) / 2;
  const bc = (b0 + b1) / 2;
  if (ac >= b0 && ac <= b1) return ac;
  if (bc >= a0 && bc <= a1) return bc;
  return (lo + hi) / 2;
}

const AVOID_CLEARANCE = 6;

/** Moves a straight segment inside its overlap range to the nearest line that
 * clears every avoid rect by the clearance; the segment stays as is if none fits. */
function dodge(
  seg: Point[],
  vertical: boolean,
  [lo, hi]: [number, number],
  avoid: Rect[],
): Point[] {
  const at = vertical ? seg[0].x : seg[0].y;
  const s0 = Math.min(...seg.map((p) => (vertical ? p.y : p.x)));
  const s1 = Math.max(...seg.map((p) => (vertical ? p.y : p.x)));
  const span = (r: Rect) =>
    vertical
      ? [r.y, r.y + r.h, r.x, r.x + r.w]
      : [r.x, r.x + r.w, r.y, r.y + r.h];
  const clears = (c: number) =>
    avoid.every((r) => {
      const [a0, a1, c0, c1] = span(r);
      return (
        a1 <= s0 ||
        a0 >= s1 ||
        c <= c0 - AVOID_CLEARANCE ||
        c >= c1 + AVOID_CLEARANCE
      );
    });
  if (clears(at)) return seg;
  const candidates = avoid
    .flatMap((r) => {
      const [, , c0, c1] = span(r);
      return [c0 - AVOID_CLEARANCE, c1 + AVOID_CLEARANCE];
    })
    .filter((c) => c >= lo && c <= hi && clears(c))
    .sort((m, n) => Math.abs(m - at) - Math.abs(n - at));
  if (candidates.length === 0) return seg;
  return seg.map((p) =>
    vertical ? { ...p, x: candidates[0] } : { ...p, y: candidates[0] },
  );
}

export function groupLabelRect(g: Rect, label: string): Rect {
  return { x: g.x + 10, y: g.y + 4, w: 16 + label.length * 6.6, h: 18 };
}

export function routeEdge(a: Rect, b: Rect, avoid: Rect[] = []): Point[] {
  const ar = a.x + a.w;
  const ab = a.y + a.h;
  const br = b.x + b.w;
  const bb = b.y + b.h;
  const xo = overlapRange(a.x, ar, b.x, br);
  const yo = overlapRange(a.y, ab, b.y, bb);
  const gapY = Math.max(b.y - ab, a.y - bb);
  const gapX = Math.max(b.x - ar, a.x - br);
  if (xo && (gapY >= 0 || !yo)) {
    const x = crossPos(a.x, ar, b.x, br, xo);
    const seg =
      b.y >= ab
        ? [
            { x, y: ab },
            { x, y: b.y },
          ]
        : [
            { x, y: a.y },
            { x, y: bb },
          ];
    return dodge(seg, true, xo, avoid);
  }
  if (yo && gapX >= 0) {
    const y = crossPos(a.y, ab, b.y, bb, yo);
    const seg =
      b.x >= ar
        ? [
            { x: ar, y },
            { x: b.x, y },
          ]
        : [
            { x: a.x, y },
            { x: br, y },
          ];
    return dodge(seg, false, yo, avoid);
  }
  const acx = a.x + a.w / 2;
  const acy = a.y + a.h / 2;
  const bcx = b.x + b.w / 2;
  const bcy = b.y + b.h / 2;
  if (gapX >= gapY) {
    return [
      { x: b.x >= ar ? ar : a.x, y: acy },
      { x: bcx, y: acy },
      { x: bcx, y: b.y >= ab ? b.y : bb },
    ];
  }
  return [
    { x: acx, y: b.y >= ab ? ab : a.y },
    { x: acx, y: bcy },
    { x: b.x >= ar ? b.x : br, y: bcy },
  ];
}

export function labelAt(points: Point[]): {
  x: number;
  y: number;
  vertical: boolean;
} {
  if (points.length === 0) return { x: 0, y: 0, vertical: false };
  if (points.length === 1) return { ...points[0], vertical: false };
  let best = 0;
  let bestLen = -1;
  for (let i = 0; i < points.length - 1; i++) {
    const len =
      Math.abs(points[i + 1].x - points[i].x) +
      Math.abs(points[i + 1].y - points[i].y);
    if (len > bestLen) {
      best = i;
      bestLen = len;
    }
  }
  const p = points[best];
  const q = points[best + 1];
  return {
    x: (p.x + q.x) / 2,
    y: (p.y + q.y) / 2,
    vertical: p.x === q.x,
  };
}

/** Owner ops replayed on a fresh file, then every unplaced node pinned where
 * the Preview would draw it, so a saved position equals what the owner saw. */
export function materializeAfter(fresh: Diagram, ops: DiagramOp[]): Diagram {
  const next = applyOps(fresh, ops);
  return materialize(next, placeNodes(next));
}

/** The op with every id it names looked up in `remap`. An op that adds
 * something keeps its own id. */
export function remapOp(op: DiagramOp, remap: Map<string, string>): DiagramOp {
  const idOf = (id: string) => remap.get(id) ?? id;
  switch (op.op) {
    case "add-node":
      return op.node.group
        ? { op: "add-node", node: { ...op.node, group: idOf(op.node.group) } }
        : op;
    case "add-group":
    case "add-note":
      return op;
    case "connect":
      return {
        op: "connect",
        edge: { ...op.edge, from: idOf(op.edge.from), to: idOf(op.edge.to) },
      };
    case "set":
      return {
        ...op,
        id: idOf(op.id),
        value: op.field === "group" ? idOf(op.value) : op.value,
      };
    default:
      return { ...op, id: idOf(op.id) };
  }
}

/** Gives an add-node, add-group, add-note or connect whose id the fresh file
 * already uses a new id and remaps the later ops that name the old one.
 * `remap` carries the renames of earlier batches in and the new ones out. */
export function reidOps(
  fresh: Diagram,
  ops: DiagramOp[],
  remap: Map<string, string> = new Map(),
): DiagramOp[] {
  const taken = allIds(fresh);
  const reserve = (id: string) => {
    if (!taken.has(id)) {
      taken.add(id);
      return id;
    }
    let n = 2;
    const slug = newId(fresh, id);
    let next = slug;
    while (taken.has(next)) next = `${slug}-${n++}`;
    taken.add(next);
    remap.set(id, next);
    return next;
  };
  return ops.map((raw): DiagramOp => {
    const op = remapOp(raw, remap);
    switch (op.op) {
      case "add-node":
        return {
          op: "add-node",
          node: { ...op.node, id: reserve(op.node.id) },
        };
      case "add-group":
        return {
          op: "add-group",
          group: { ...op.group, id: reserve(op.group.id) },
        };
      case "add-note":
        return {
          op: "add-note",
          note: { ...op.note, id: reserve(op.note.id) },
        };
      case "connect":
        return {
          op: "connect",
          edge: { ...op.edge, id: reserve(op.edge.id) },
        };
      default:
        return op;
    }
  });
}

type Side = "t" | "r" | "b" | "l";
const SIDE_ORDER: readonly Side[] = ["r", "b", "l", "t"];
const LANE_OUT = 24;
const LANE_SHIFTS = [0, 12, -12, 24, -24, 36, -36];
const CLEARANCE = 12;
const MIN_CLEARANCE = 4;
const LANE_REACH = 40;
const CRAMPED = 400;
const PORT_TRIES = [0.5, 0.25, 0.75, 0.15, 0.85];
const CROSSING = 500;
const PAIR_OFFSET = 8;
const CYLINDER_CAP = 6;
const FACING = 0.1;
const SHARE_COST = 0.6;
const MAX_EXACT = 6;
const SAMPLE = 2;
const DEPTH = 0.5;
const SHARED_MAX = 4;
const MIN_JOG = 16;
const ALIGN_INSET = 8;

/** The sides routeEdge would use: a leaves through the first, b is entered
 * through the second. */
function sidesFor(a: Rect, b: Rect): [Side, Side] {
  const ar = a.x + a.w;
  const ab = a.y + a.h;
  const br = b.x + b.w;
  const bb = b.y + b.h;
  const xo = overlapRange(a.x, ar, b.x, br);
  const yo = overlapRange(a.y, ab, b.y, bb);
  const gapY = Math.max(b.y - ab, a.y - bb);
  const gapX = Math.max(b.x - ar, a.x - br);
  if (xo && (gapY >= 0 || !yo)) return b.y >= ab ? ["b", "t"] : ["t", "b"];
  if (yo && gapX >= 0) return b.x >= ar ? ["r", "l"] : ["l", "r"];
  if (gapX >= gapY) {
    return [b.x >= ar ? "r" : "l", b.y >= ab ? "t" : "b"];
  }
  return [b.y >= ab ? "b" : "t", b.x >= ar ? "l" : "r"];
}

const centreOf = (r: Rect): Point => ({ x: r.x + r.w / 2, y: r.y + r.h / 2 });

function portAt(r: Rect, side: Side, f: number): Point {
  switch (side) {
    case "t":
      return { x: r.x + r.w * f, y: r.y };
    case "b":
      return { x: r.x + r.w * f, y: r.y + r.h };
    case "l":
      return { x: r.x, y: r.y + r.h * f };
    case "r":
      return { x: r.x + r.w, y: r.y + r.h * f };
  }
}

/** `p`, a point on the side of the bounding box, moved along the side's
 * normal onto the outline the shape really draws there. */
function onOutline(r: Rect, shape: unknown, p: Point, side: Side): Point {
  const cx = r.x + r.w / 2;
  const cy = r.y + r.h / 2;
  const clamp = (v: number, lo: number, hi: number) =>
    Math.min(Math.max(v, lo), hi);
  const vertical = side === "t" || side === "b";
  switch (shape) {
    case "diamond": {
      // The point of the diamond edge on this side; the vertex at the middle.
      if (vertical) {
        const x = clamp(p.x, r.x, r.x + r.w);
        const dy = (r.h / 2) * (1 - Math.abs(x - cx) / (r.w / 2));
        return { x, y: side === "t" ? cy - dy : cy + dy };
      }
      const y = clamp(p.y, r.y, r.y + r.h);
      const dx = (r.w / 2) * (1 - Math.abs(y - cy) / (r.h / 2));
      return { x: side === "l" ? cx - dx : cx + dx, y };
    }
    case "circle": {
      const R = r.w / 2;
      if (vertical) {
        const x = clamp(p.x, cx - R, cx + R);
        const dy = Math.sqrt(R * R - (x - cx) ** 2);
        return { x, y: side === "t" ? cy - dy : cy + dy };
      }
      const y = clamp(p.y, cy - R, cy + R);
      const dx = Math.sqrt(R * R - (y - cy) ** 2);
      return { x: side === "l" ? cx - dx : cx + dx, y };
    }
    case "pill": {
      const R = r.h / 2;
      if (vertical) return { x: clamp(p.x, r.x + R, r.x + r.w - R), y: p.y };
      const y = clamp(p.y, cy - R, cy + R);
      const dx = R - Math.sqrt(R * R - (y - cy) ** 2);
      return { x: side === "l" ? r.x + dx : r.x + r.w - dx, y };
    }
    case "cylinder":
      return side === "t" ? { x: p.x, y: r.y + CYLINDER_CAP } : p;
    default:
      return p;
  }
}

/** Sides of a diamond for its ends: each end takes a side that faces its far
 * end, a different one while enough are free; ends that must share a side are
 * spread along it. `others` are the centres at the far ends. */
function diamondSides(from: Point, others: Point[]): Side[] {
  const scores = others.map((o) => {
    const dx = o.x - from.x;
    const dy = o.y - from.y;
    const len = Math.hypot(dx, dy) || 1;
    return { t: -dy / len, b: dy / len, l: -dx / len, r: dx / len };
  });
  const facing = scores.map((sc) => {
    const ranked = [...SIDE_ORDER].sort((m, n) => sc[n] - sc[m]);
    const ok = ranked.filter((side) => sc[side] > FACING);
    return ok.length > 0 ? ok : [ranked[0]];
  });
  const best = facing.map((f) => f[0]);
  if (others.length > MAX_EXACT) return best;
  let top = -Infinity;
  let pick: Side[] = best;
  const walk = (i: number, used: Side[], sum: number) => {
    if (i === scores.length) {
      if (sum > top + 1e-9) {
        top = sum;
        pick = used;
      }
      return;
    }
    for (const s of facing[i]) {
      const cost = used.includes(s) ? SHARE_COST : 0;
      walk(i + 1, [...used, s], sum + scores[i][s] - cost);
    }
  };
  walk(0, [], 0);
  return pick;
}

type Lane = (horizontal: boolean, c: number, lo: number, hi: number) => number;

const turn = (p: Point): Point => ({ x: p.y, y: p.x });
const turnSide = (s: Side): Side =>
  ({ t: "l", b: "r", l: "t", r: "b" })[s] as Side;

/** An orthogonal path from p (leaving through s) to q (entering through t). */
function connect(p: Point, s: Side, q: Point, t: Side, lane: Lane): Point[] {
  const vs = s === "t" || s === "b";
  const vt = t === "t" || t === "b";
  if (vs && vt) {
    const ds = s === "b" ? 1 : -1;
    const dt = t === "b" ? 1 : -1;
    const facing = s !== t && (q.y - p.y) * ds >= 0;
    if (facing) {
      const lo = Math.min(p.x, q.x);
      const hi = Math.max(p.x, q.x);
      const y = lane(true, (p.y + q.y) / 2, lo, hi);
      return [p, { x: p.x, y }, { x: q.x, y }, q];
    }
    const y1 = p.y + ds * LANE_OUT;
    const y2 = q.y + dt * LANE_OUT;
    if (s === t) {
      const y = lane(
        true,
        ds > 0 ? Math.max(y1, y2) : Math.min(y1, y2),
        Math.min(p.x, q.x),
        Math.max(p.x, q.x),
      );
      return [p, { x: p.x, y }, { x: q.x, y }, q];
    }
    const mx = p.x === q.x ? p.x + 60 : (p.x + q.x) / 2;
    const x = lane(false, mx, Math.min(y1, y2), Math.max(y1, y2));
    return [
      p,
      { x: p.x, y: y1 },
      { x, y: y1 },
      { x, y: y2 },
      { x: q.x, y: y2 },
      q,
    ];
  }
  if (!vs && !vt) {
    return connect(turn(p), turnSide(s), turn(q), turnSide(t), (h, c, lo, hi) =>
      lane(!h, c, lo, hi),
    ).map(turn);
  }
  return vs ? [p, { x: p.x, y: q.y }, q] : [p, { x: q.x, y: p.y }, q];
}

function simplify(pts: Point[]): Point[] {
  const out: Point[] = [];
  for (const p of pts) {
    const last = out[out.length - 1];
    if (last && last.x === p.x && last.y === p.y) continue;
    out.push(p);
  }
  for (let i = out.length - 2; i > 0; i--) {
    const a = out[i - 1];
    const b = out[i];
    const c = out[i + 1];
    if ((a.x === b.x && b.x === c.x) || (a.y === b.y && b.y === c.y)) {
      out.splice(i, 1);
    }
  }
  return out;
}

/** Whether `p` is deeper than DEPTH inside what a block really draws. */
function insideShape(r: Rect, shape: unknown, p: Point): boolean {
  const cx = r.x + r.w / 2;
  const cy = r.y + r.h / 2;
  const dx = Math.abs(p.x - cx);
  const dy = Math.abs(p.y - cy);
  switch (shape) {
    case "diamond":
      return dx / (r.w / 2 - DEPTH) + dy / (r.h / 2 - DEPTH) < 1;
    case "circle":
      return Math.hypot(dx, dy) < r.w / 2 - DEPTH;
    case "pill": {
      const R = r.h / 2;
      const flat = r.w / 2 - R;
      return Math.hypot(Math.max(dx - flat, 0), dy) < R - DEPTH;
    }
    default:
      return dx < r.w / 2 - DEPTH && dy < r.h / 2 - DEPTH;
  }
}

/** Whether a polyline runs through the inside of a block. */
function crossesBlock(pts: Point[], r: Rect, shape: unknown): boolean {
  for (let i = 1; i < pts.length; i++) {
    const [p, q] = [pts[i - 1], pts[i]];
    const len = Math.hypot(q.x - p.x, q.y - p.y);
    const n = Math.max(1, Math.ceil(len / SAMPLE));
    for (let k = 0; k <= n; k++) {
      const t = k / n;
      const at = { x: p.x + (q.x - p.x) * t, y: p.y + (q.y - p.y) * t };
      if (insideShape(r, shape, at)) return true;
    }
  }
  return false;
}

const inflate = (r: Rect, by: number): Rect => ({
  x: r.x - by,
  y: r.y - by,
  w: r.w + 2 * by,
  h: r.h + 2 * by,
});

/** Whether a straight run (a horizontal one at y=c from lo to hi, else a
 * vertical one at x=c) passes through the inside of `r`. */
function rectHit(
  r: Rect,
  horizontal: boolean,
  c: number,
  lo: number,
  hi: number,
): boolean {
  return horizontal
    ? r.y < c && c < r.y + r.h && lo < r.x + r.w && hi > r.x
    : r.x < c && c < r.x + r.w && lo < r.y + r.h && hi > r.y;
}

function hitsAny(pts: Point[], rects: Rect[]): boolean {
  for (let i = 1; i < pts.length; i++) {
    const [p, q] = [pts[i - 1], pts[i]];
    const horizontal = p.y === q.y;
    const [u, v] = horizontal ? (["x", "y"] as const) : (["y", "x"] as const);
    const lo = Math.min(p[u], q[u]);
    const hi = Math.max(p[u], q[u]);
    if (rects.some((r) => rectHit(r, horizontal, p[v], lo, hi))) return true;
  }
  return false;
}

/** Every segment horizontal or vertical: a slanted one (ports in different
 * columns after snapping to an outline) gets a bend halfway along its longer
 * axis. */
function squareUp(pts: Point[]): Point[] {
  const out: Point[] = [pts[0]];
  for (let i = 1; i < pts.length; i++) {
    const [p, q] = [out[out.length - 1], pts[i]];
    if (p.x !== q.x && p.y !== q.y) {
      if (Math.abs(q.y - p.y) >= Math.abs(q.x - p.x)) {
        const y = (p.y + q.y) / 2;
        out.push({ x: p.x, y }, { x: q.x, y });
      } else {
        const x = (p.x + q.x) / 2;
        out.push({ x, y: p.y }, { x, y: q.y });
      }
    }
    out.push(q);
  }
  return out;
}

/** True when two polylines start or end on the same point. */
function meets(a: Point[], b: Point[]): boolean {
  const ends = (l: Point[]) => [l[0], l[l.length - 1]];
  return ends(a).some((p) =>
    ends(b).some((q) => Math.hypot(p.x - q.x, p.y - q.y) < SHARED_MAX),
  );
}

/** How many times two polylines cross at a right angle. */
function crossings(a: Point[], b: Point[]): number {
  let n = 0;
  for (let i = 1; i < a.length; i++) {
    for (let j = 1; j < b.length; j++) {
      const [p, q, r, t] = [a[i - 1], a[i], b[j - 1], b[j]];
      const h = p.y === q.y;
      if (h === (r.y === t.y)) continue;
      const [hp, hq, vr, vt] = h ? [p, q, r, t] : [r, t, p, q];
      const x = vr.x;
      const y = hp.y;
      if (
        Math.min(hp.x, hq.x) < x &&
        x < Math.max(hp.x, hq.x) &&
        Math.min(vr.y, vt.y) < y &&
        y < Math.max(vr.y, vt.y)
      ) {
        n++;
      }
    }
  }
  return n;
}

/** True when two polylines run along one line for more than `limit`. */
function runsShare(a: Point[], b: Point[], limit: number): boolean {
  for (let i = 1; i < a.length; i++) {
    for (let j = 1; j < b.length; j++) {
      const [p, q, r, t] = [a[i - 1], a[i], b[j - 1], b[j]];
      const horizontal = p.y === q.y;
      if (horizontal !== (r.y === t.y)) continue;
      const [u, v] = horizontal ? (["x", "y"] as const) : (["y", "x"] as const);
      if (Math.abs(p[v] - r[v]) > limit) continue;
      const lo = Math.max(Math.min(p[u], q[u]), Math.min(r[u], t[u]));
      const hi = Math.min(Math.max(p[u], q[u]), Math.max(r[u], t[u]));
      if (hi - lo > limit) return true;
    }
  }
  return false;
}

/** How many runs of a polyline pass through the inside of a rect. */
function clashes(pts: Point[], rects: Rect[]): number {
  let n = 0;
  for (let i = 1; i < pts.length; i++) {
    const [p, q] = [pts[i - 1], pts[i]];
    const horizontal = p.y === q.y;
    const [u, v] = horizontal ? (["x", "y"] as const) : (["y", "x"] as const);
    const lo = Math.min(p[u], q[u]);
    const hi = Math.max(p[u], q[u]);
    if (rects.some((r) => rectHit(r, horizontal, p[v], lo, hi))) n++;
  }
  return n;
}

/** How many runs between two bends are shorter than MIN_JOG: a sideways
 * step too small to read as anything but a flaw. */
function jogs(pts: Point[]): number {
  let n = 0;
  for (let i = 1; i < pts.length - 2; i++) {
    const len =
      Math.abs(pts[i + 1].x - pts[i].x) + Math.abs(pts[i + 1].y - pts[i].y);
    if (len < MIN_JOG) n++;
  }
  return n;
}

const SIDE_STEP: Record<Side, Point> = {
  t: { x: 0, y: -1 },
  b: { x: 0, y: 1 },
  l: { x: -1, y: 0 },
  r: { x: 1, y: 0 },
};

/** Whether the run from `p` to `q` leaves the block `r` along the normal of
 * a side `p` lies on, not along the block. A port off every side is let be. */
function leaves(r: Rect, p: Point, q: Point): boolean {
  const on: Record<Side, boolean> = {
    t: p.y === r.y,
    b: p.y === r.y + r.h,
    l: p.x === r.x,
    r: p.x === r.x + r.w,
  };
  const sides = SIDE_ORDER.filter((side) => on[side]);
  return (
    !sides.length ||
    sides.some(
      (side) =>
        (q.x - p.x) * SIDE_STEP[side].x + (q.y - p.y) * SIDE_STEP[side].y > 0,
    )
  );
}

/** Whether both ends of a polyline leave their blocks along a side's normal. */
function squareEnds(pts: Point[], a: Rect, b: Rect): boolean {
  const n = pts.length;
  return leaves(a, pts[0], pts[1]) && leaves(b, pts[n - 1], pts[n - 2]);
}

/** Whether `p` is one of the four vertices of the box `r`'s diamond. */
function atVertex(r: Rect, p: Point): boolean {
  const cx = r.x + r.w / 2;
  const cy = r.y + r.h / 2;
  return (
    (p.x === cx && (p.y === r.y || p.y === r.y + r.h)) ||
    (p.y === cy && (p.x === r.x || p.x === r.x + r.w))
  );
}

/** Routes the edges of `d` together. Each edge leaves and enters through the
 * side routeEdge would pick; edges sharing a side of a node get their own
 * ports along it, ordered by the far end so they do not cross; a diamond uses
 * its vertices, a different one per edge while one is free. Middle runs that
 * would share a line are moved apart, and away from the `avoid` rects. */
export function routeEdges(
  d: Diagram,
  pos: Map<string, Point>,
  avoid: Rect[] = [],
): Map<string, Point[]> {
  const nodes = new Map(d.nodes.map((n) => [n.id, n]));
  const rects = new Map<string, Rect>();
  for (const n of d.nodes) {
    const p = pos.get(n.id);
    if (p) rects.set(n.id, nodeRect(p, n));
  }
  const edges = d.edges.filter(
    (e) => rects.has(e.from) && rects.has(e.to) && e.from !== e.to,
  );
  // End k of edge i: k=0 leaves `from`, k=1 enters `to`.
  const sides: Side[][] = edges.map((e) =>
    sidesFor(rects.get(e.from) as Rect, rects.get(e.to) as Rect),
  );
  const ends = (id: string) =>
    edges.flatMap((e, i) =>
      [e.from, e.to].flatMap((nid, k) => (nid === id ? [{ i, k }] : [])),
    );
  for (const [id, node] of nodes) {
    if (node.shape !== "diamond" || !rects.has(id)) continue;
    const list = ends(id);
    const mine = centreOf(rects.get(id) as Rect);
    const picked = diamondSides(
      mine,
      list.map(({ i, k }) =>
        centreOf(rects.get(k === 0 ? edges[i].to : edges[i].from) as Rect),
      ),
    );
    list.forEach(({ i, k }, j) => (sides[i][k] = picked[j]));
  }

  const port: Point[][] = edges.map(() => []);
  // An edge between plain blocks stays as routeEdge draws it unless that
  // line would share a stretch with another edge's.
  const plain = edges.map(
    (e) =>
      nodes.get(e.from)?.shape !== "diamond" &&
      nodes.get(e.to)?.shape !== "diamond",
  );
  const direct = edges.map((e) =>
    routeEdge(rects.get(e.from) as Rect, rects.get(e.to) as Rect, avoid),
  );
  // Blocks a run must stay clear of: every block that is not one of its ends.
  const others = (i: number) =>
    [...rects]
      .filter(([id]) => id !== edges[i].from && id !== edges[i].to)
      .map(([, r]) => inflate(r, CLEARANCE));
  // An antiparallel pair on facing sides: two straight lines either side of
  // the shared centre line.
  edges.forEach((e, i) => {
    const j = edges.findIndex(
      (f, k) => k > i && f.from === e.to && f.to === e.from,
    );
    const a = direct[i];
    const b = direct[j];
    if (j < 0 || !plain[i] || !plain[j] || a.length !== 2 || b.length !== 2) {
      return;
    }
    const horizontal = a[0].y === a[1].y;
    if (horizontal !== (b[0].y === b[1].y)) return;
    const u = horizontal ? "y" : "x";
    const ra = rects.get(e.from) as Rect;
    const rb = rects.get(e.to) as Rect;
    const lo = Math.max(horizontal ? ra.y : ra.x, horizontal ? rb.y : rb.x);
    const hi = Math.min(
      horizontal ? ra.y + ra.h : ra.x + ra.w,
      horizontal ? rb.y + rb.h : rb.x + rb.w,
    );
    const c = a[0][u];
    if (c - PAIR_OFFSET < lo || c + PAIR_OFFSET > hi) return;
    direct[i] = a.map((q) => ({ ...q, [u]: c - PAIR_OFFSET }));
    direct[j] = b.map((q) => ({ ...q, [u]: c + PAIR_OFFSET }));
  });
  const keep = edges.map(
    (_, i) =>
      plain[i] &&
      !hitsAny(direct[i], others(i)) &&
      direct.every(
        (other, j) => j === i || !runsShare(direct[i], other, SHARED_MAX),
      ),
  );
  for (const [id, r] of rects) {
    const bySide = new Map<Side, { i: number; k: number }[]>();
    // Where kept lines already meet a side, as fractions along it.
    const fixed = new Map<Side, number[]>();
    for (const end of ends(id)) {
      const s = sides[end.i][end.k];
      if (keep[end.i]) {
        const line = direct[end.i];
        const pt = end.k === 0 ? line[0] : line[line.length - 1];
        const f =
          s === "t" || s === "b" ? (pt.x - r.x) / r.w : (pt.y - r.y) / r.h;
        fixed.set(s, [...(fixed.get(s) ?? []), f]);
        continue;
      }
      bySide.set(s, [...(bySide.get(s) ?? []), end]);
    }
    for (const [side, list] of bySide) {
      const vertical = side === "t" || side === "b";
      const far = ({ i, k }: { i: number; k: number }) => {
        const c = centreOf(
          rects.get(k === 0 ? edges[i].to : edges[i].from) as Rect,
        );
        return vertical ? c.x : c.y;
      };
      list.sort((m, n) => far(m) - far(n) || m.i - n.i);
      // Slots along the side, less the one nearest each kept line.
      const taken = fixed.get(side) ?? [];
      const total = list.length + taken.length;
      let slots = Array.from(
        { length: total },
        (_, j) => (j + 1) / (total + 1),
      );
      for (const f of taken) {
        const nearest = slots.reduce((m, v) =>
          Math.abs(v - f) < Math.abs(m - f) ? v : m,
        );
        slots = slots.filter((v) => v !== nearest);
      }
      list.forEach((end, j) => {
        port[end.i][end.k] = onOutline(
          r,
          nodes.get(id)?.shape,
          portAt(r, side, slots[j]),
          side,
        );
      });
    }
  }

  const placed: { horizontal: boolean; c: number; lo: number; hi: number }[] =
    [];
  const shared = (horizontal: boolean, c: number, lo: number, hi: number) =>
    placed.some(
      (s) =>
        s.horizontal === horizontal &&
        Math.abs(s.c - c) <= SHARED_MAX &&
        Math.min(s.hi, hi) - Math.max(s.lo, lo) > SHARED_MAX,
    );
  const register = (pts: Point[]) => {
    for (let k = 1; k < pts.length; k++) {
      const [m, n] = [pts[k - 1], pts[k]];
      const horizontal = m.y === n.y;
      const [u, v] = horizontal ? (["x", "y"] as const) : (["y", "x"] as const);
      placed.push({
        horizontal,
        c: m[v],
        lo: Math.min(m[u], n[u]),
        hi: Math.max(m[u], n[u]),
      });
    }
  };
  let clear: Rect[] = [];
  // A run keeps CLEARANCE from other blocks; where two blocks stand closer
  // than that, it only stays out of them.
  const routed: Point[][] = [];
  const makeLane =
    (record: boolean): Lane =>
    (horizontal, c, lo, hi) => {
      const pick = (walls: Rect[], reach: number) => {
        const blocked = (v: number) =>
          [...avoid, ...walls].some((r) => rectHit(r, horizontal, v, lo, hi));
        const edgesOf = walls
          .flatMap((r) => (horizontal ? [r.y, r.y + r.h] : [r.x, r.x + r.w]))
          .sort((m, n) => Math.abs(m - c) - Math.abs(n - c));
        const cross = (v: number) => {
          const run = horizontal
            ? [
                { x: lo, y: v },
                { x: hi, y: v },
              ]
            : [
                { x: v, y: lo },
                { x: v, y: hi },
              ];
          return routed.reduce((n, o) => n + crossings(run, o), 0);
        };
        const ok = [...LANE_SHIFTS.map((s) => c + s), ...edgesOf].filter(
          (v) =>
            Math.abs(v - c) <= reach &&
            !shared(horizontal, v, lo, hi) &&
            !blocked(v),
        );
        // Fewest crossings first, then the nearest line.
        return ok.sort(
          (m, n) => cross(m) - cross(n) || Math.abs(m - c) - Math.abs(n - c),
        )[0];
      };
      const raw = clear.map((r) => inflate(r, MIN_CLEARANCE - CLEARANCE));
      const at =
        pick(clear, LANE_REACH) ??
        pick(raw, LANE_REACH) ??
        pick(clear, Infinity) ??
        pick(raw, Infinity) ??
        c;
      if (record) placed.push({ horizontal, c: at, lo, hi });
      return at;
    };
  const lane = makeLane(true);
  const dry = makeLane(false);
  const crossesAny = (pts: Point[]) =>
    [...rects].some(([id, r]) => crossesBlock(pts, r, nodes.get(id)?.shape));
  const length = (pts: Point[]) =>
    pts
      .slice(1)
      .reduce(
        (n, q, k) => n + Math.abs(q.x - pts[k].x) + Math.abs(q.y - pts[k].y),
        0,
      );
  // A port on a side that no drawn line already ends at.
  const freePort = (id: string, side: Side, drawn: Point[][]) => {
    const at = (f: number) =>
      onOutline(
        rects.get(id) as Rect,
        nodes.get(id)?.shape,
        portAt(rects.get(id) as Rect, side, f),
        side,
      );
    const ends = drawn.flatMap((o) => [o[0], o[o.length - 1]]);
    const taken = (p: Point) =>
      ends.some((q) => Math.hypot(p.x - q.x, p.y - q.y) < 2 * SHARED_MAX);
    const f = PORT_TRIES.find((v) => !taken(at(v)));
    return at(f ?? 0.5);
  };
  /** A route through no block: the side pair with the shortest path that
   * neither crosses a block nor runs along an edge already drawn. */
  const costOf = (pts: Point[], drawn: Point[][]) =>
    length(pts) +
    (hitsAny(pts, clear) ? CRAMPED : 0) +
    CROSSING * drawn.reduce((n, o) => n + crossings(pts, o), 0) +
    (drawn.some((o) => runsShare(pts, o, SHARED_MAX) || meets(pts, o))
      ? 10000
      : 0);
  const detour = (
    e: DiagramEdge,
    drawn: Point[][],
  ): { pts: Point[]; cost: number } | null => {
    let best: { pts: Point[]; cost: number } | null = null;
    for (const s of SIDE_ORDER) {
      for (const t of SIDE_ORDER) {
        const pts = simplify(
          connect(
            freePort(e.from, s, drawn),
            s,
            freePort(e.to, t, drawn),
            t,
            dry,
          ),
        );
        if (crossesAny(pts)) continue;
        const cost = costOf(pts, drawn);
        if (!best || cost < best.cost) best = { pts, cost };
      }
    }
    return best;
  };

  const out = new Map<string, Point[]>();
  const order = edges.map((_, i) => i).sort((m, n) => +keep[n] - +keep[m]);
  order.forEach((i) => {
    const e = edges[i];
    if (keep[i]) {
      const pts = direct[i];
      register(pts);
      const snapped = [...pts];
      const last = pts.length - 1;
      snapped[0] = onOutline(
        rects.get(e.from) as Rect,
        nodes.get(e.from)?.shape,
        pts[0],
        sides[i][0],
      );
      snapped[last] = onOutline(
        rects.get(e.to) as Rect,
        nodes.get(e.to)?.shape,
        pts[last],
        sides[i][1],
      );
      out.set(e.id, snapped);
      routed.push(snapped);
      return;
    }
    clear = others(i);
    const pts = simplify(
      connect(port[i][0], sides[i][0], port[i][1], sides[i][1], lane),
    );
    const around = crossesAny(pts) ? detour(e, [...out.values()]) : null;
    if (around) register(around.pts);
    out.set(e.id, around?.pts ?? pts);
    routed.push(around?.pts ?? pts);
  });
  // Every line was drawn before the later ones: look again at each, now that
  // all the others are known, and keep the cheaper of the two.
  for (const i of order) {
    if (keep[i]) continue;
    const e = edges[i];
    const mine = out.get(e.id) as Point[];
    const drawn = [...out.values()].filter((o) => o !== mine);
    placed.length = 0;
    routed.length = 0;
    for (const o of drawn) {
      register(o);
      routed.push(o);
    }
    clear = others(i);
    const pts = simplify(
      connect(port[i][0], sides[i][0], port[i][1], sides[i][1], lane),
    );
    const plainCost = crossesAny(pts) ? Infinity : costOf(pts, drawn);
    const other = detour(e, drawn);
    const again =
      other && other.cost < plainCost ? other : { pts, cost: plainCost };
    const next = again && again.cost < costOf(mine, drawn) ? again.pts : mine;
    register(next);
    routed.push(next);
    out.set(e.id, next);
  }
  // Last look at each line: one that squeezes past a block closer than the
  // clearance, jogs a few pixels sideways or leaves a diamond off a vertex is
  // drawn again from every side and port, and the best candidate replaces it.
  type Score = number[];
  const score = (pts: Point[], i: number, drawn: Point[][]): Score => [
    clashes(pts, [...others(i), ...avoid]),
    jogs(pts),
    pts.length,
    drawn.reduce((n, o) => n + crossings(pts, o), 0),
    length(pts),
  ];
  const better = (a: Score, b: Score) => {
    for (let k = 0; k < a.length; k++) if (a[k] !== b[k]) return a[k] < b[k];
    return false;
  };
  const portsOf = (id: string, side: Side) => {
    const r = rects.get(id) as Rect;
    const shape = nodes.get(id)?.shape;
    const fs = shape === "diamond" ? [0.5] : PORT_TRIES;
    return fs.map((f) => onOutline(r, shape, portAt(r, side, f), side));
  };
  // The port of `id` on `side` lined up with `at` (same x for a top or bottom
  // side, same y for a left or right one), when the block is wide enough.
  const aligned = (id: string, side: Side, at: Point): Point | null => {
    const r = rects.get(id) as Rect;
    const shape = nodes.get(id)?.shape;
    const vertical = side === "t" || side === "b";
    const f = vertical ? (at.x - r.x) / r.w : (at.y - r.y) / r.h;
    const room = ALIGN_INSET / (vertical ? r.w : r.h);
    if (f < room || f > 1 - room) return null;
    const p = onOutline(r, shape, portAt(r, side, f), side);
    return shape === "diamond" && !atVertex(r, p) ? null : p;
  };
  for (const i of order) {
    const e = edges[i];
    const mine = squareUp(out.get(e.id) as Point[]);
    const flawed = [e.from, e.to].some(
      (id, k) =>
        nodes.get(id)?.shape === "diamond" &&
        !atVertex(
          rects.get(id) as Rect,
          k === 0 ? mine[0] : mine[mine.length - 1],
        ),
    );
    const drawn = [...out.entries()]
      .filter(([id]) => id !== e.id)
      .map(([, o]) => o);
    const now = score(mine, i, drawn);
    const ra = rects.get(e.from) as Rect;
    const rb = rects.get(e.to) as Rect;
    const askew = !squareEnds(mine, ra, rb);
    if (!flawed && !askew && now[0] === 0 && now[1] === 0) continue;
    placed.length = 0;
    routed.length = 0;
    for (const o of drawn) {
      register(o);
      routed.push(o);
    }
    clear = others(i);
    let best = flawed || askew ? null : { pts: mine, at: now };
    const tryOut = (s: Side, p: Point, t: Side, q: Point) => {
      const pts = simplify(connect(p, s, q, t, dry));
      if (pts.length < 2 || !squareEnds(pts, ra, rb) || crossesAny(pts)) return;
      if (drawn.some((o) => runsShare(pts, o, SHARED_MAX) || meets(pts, o))) {
        return;
      }
      const at = score(pts, i, drawn);
      if (!best || better(at, best.at)) best = { pts, at };
    };
    for (const s of SIDE_ORDER) {
      for (const t of SIDE_ORDER) {
        const as = portsOf(e.from, s);
        const bs = portsOf(e.to, t);
        for (const p of as) {
          for (const q of bs) tryOut(s, p, t, q);
          const q = aligned(e.to, t, p);
          if (q) tryOut(s, p, t, q);
        }
        for (const q of bs) {
          const p = aligned(e.from, s, q);
          if (p) tryOut(s, p, t, q);
        }
      }
    }
    if (best) out.set(e.id, (best as { pts: Point[] }).pts);
  }
  for (const [id, pts] of out) out.set(id, squareUp(pts));
  return out;
}
