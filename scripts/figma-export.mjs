#!/usr/bin/env node
// Normalise a Figma node dump into a compact snapshot.
//
//   node scripts/figma-export.mjs <raw.json> [--out dir] [--root <layer name>]
//
// Input: the JSON of `figma_get_component_for_development_deep`
// (`{component: {...tree}}`). The dump carries sizes and auto-layout
// settings but no x/y, so positions are computed here from padding, gap and
// alignment. A child of a frame without auto-layout has no position in the
// dump: it is placed at the parent's origin and marked `approx: true`.
//
// Output: `<out>/<raw file name>.json` (default
// concepts/figma/export/snapshots, gitignored). Every `box` is relative to
// the content root, so it compares 1:1 with a measurement of the running app.
//
// Content root: the dump's top frame is the whole window. The app shell is
// stripped by walking down from the top: while the current frame has a shell
// child, the walk enters its largest non-shell child. A shell child is a layer
// named `Shell/...`, `titlebar` or `sidebar` (any depth), or, in the top two
// levels only, a full-width bar at most 64px tall (title bar) or a column of at
// most 30% width and at least 90% height in a horizontal frame (sidebar).
// Leaf layers (a scrim rectangle) are never content. The walk stops at the
// first frame without shell children and at depth 3.
// `--root <layer name>` skips detection and uses the first layer of that name.
//
// Colours: a bound Figma variable name maps to a CSS custom property in
// src/styles/tokens.css through TOKEN_MAP below. A name with no counterpart
// (or `null` in the table) is reported as UNMAPPED: those are the items for
// the owner to extend the vocabulary with. An unbound colour is matched to a
// token by its exact value; if none matches it stays a raw `rgb(...)` and is
// reported as UNBOUND.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "..");

// Figma variable (collection "sushiAI tokens") -> CSS custom property.
// `null` = known to have no counterpart in tokens.css.
export const TOKEN_MAP = {
  "bg/app": "--bg-app",
  "bg/sunken": "--bg-sunken",
  "bg/raised": "--bg-raised",
  "bg/hover": "--bg-hover",
  "bg/selected": "--bg-selected",
  "bg/elevated": "--bg-elevated",
  "bg/control": "--bg-control",
  "bg/count": "--bg-count",
  "bg/dialog": "--bg-dialog",
  // The bar and shell surface: the same value as the dialog surface.
  "bg/shell": "--bg-dialog",
  "line/default": "--line",
  "line/pane": "--line-pane",
  "line/divider": "--line-divider",
  "line/strong": "--line-strong",
  "line/selected": "--line-selected",
  "text/default": "--text",
  "text/muted": "--text-muted",
  "text/faint": "--text-faint",
  "text/nav": "--text-nav",
  // Icons are drawn in the nav grey.
  "text/icon": "--text-nav",
  "accent/default": "--accent",
  "accent/ink": "--accent-ink",
  "accent/blue": "--accent-blue",
  "accent/violet": "--accent-violet",
  // No counterpart: agent marks are image assets (Icon in src/PanelIcon.tsx),
  // so a colour token would have no consumer.
  "accent/claude": null,
  focus: "--focus",
  "tone/ok": "--tone-ok",
  "tone/ok-bg": "--tone-ok-bg",
  "tone/info": "--tone-info",
  "tone/warning": "--tone-warning",
  "tone/warning-bg": "--tone-warning-bg",
  "tone/danger": "--tone-danger",
  "tone/danger-bg": "--tone-danger-bg",
};

// Layers that draw a rendered web page inside a mock-up (the Browser pane
// behind a dialog). Their colours belong to the page, not to the app, so they
// are kept raw and not reported as UNBOUND.
const PAGE_CONTENT_ROLES = new Set(["viewport"]);

const round = (n) => Math.round((Number(n) || 0) * 100) / 100;
const rgbString = (c) =>
  `rgb(${Math.round(c.r * 255)}, ${Math.round(c.g * 255)}, ${Math.round(c.b * 255)})`;

/** Parse the colour tokens of a tokens.css text: {"--name": "rgb(r, g, b)"}. */
export function parseTokens(css) {
  const raw = {};
  for (const m of css.matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/gi))
    raw[m[1]] = m[2].trim();
  const resolve = (value, depth = 0) => {
    const ref = /^var\((--[a-z0-9-]+)\)$/i.exec(value);
    if (ref && depth < 5 && raw[ref[1]]) return resolve(raw[ref[1]], depth + 1);
    const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(value);
    if (!hex) return null;
    let h = hex[1];
    if (h.length === 3) h = [...h].map((x) => x + x).join("");
    const n = parseInt(h, 16);
    return `rgb(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255})`;
  };
  const names = Object.keys(raw);
  const colors = {};
  for (const name of names) {
    const value = resolve(raw[name]);
    if (value) colors[name] = value;
  }
  return { names: new Set(names), colors };
}

export function loadTokens(
  file = path.join(repoRoot, "src/styles/tokens.css"),
) {
  return parseTokens(fs.readFileSync(file, "utf8"));
}

const paintsOf = (node, key) =>
  (node[key] ?? [])
    .map((paint, index) => ({ paint, index }))
    .filter(
      ({ paint }) =>
        paint.type === "SOLID" &&
        paint.visible !== false &&
        paint.opacity !== 0 &&
        paint.color,
    );

function collectIds(node, ids) {
  for (const key of ["fills", "strokes"])
    for (const v of node.boundVariables?.[key] ?? [])
      if (v?.id && v?.name) ids.set(v.id, v.name);
  for (const child of node.children ?? []) collectIds(child, ids);
}

const isVisible = (n) => n.visible !== false && !n._hidden;
const kidsOf = (n) => (n.children ?? []).filter(isVisible);

function findByName(node, name) {
  if (node.name === name) return node;
  for (const child of kidsOf(node)) {
    const hit = findByName(child, name);
    if (hit) return hit;
  }
  return null;
}

const SHELL_NAME = /^(shell\/|titlebar$|title bar$|sidebar$)/i;

function isShellChild(parent, child, depth) {
  if (SHELL_NAME.test(child.name)) return true;
  if (depth >= 2) return false;
  if (child.width >= parent.width * 0.9 && child.height <= 64) return true;
  return (
    parent.layoutMode === "HORIZONTAL" &&
    child.width <= parent.width * 0.3 &&
    child.height >= parent.height * 0.9
  );
}

/** Walk down from the top frame past the app shell; returns the content root. */
export function detectRoot(top, rootName) {
  if (rootName) {
    const hit = findByName(top, rootName);
    if (!hit) throw new Error(`--root: no layer named "${rootName}"`);
    return hit;
  }
  let node = top;
  for (let depth = 0; depth < 3; depth += 1) {
    const kids = kidsOf(node).filter((c) => c.type !== "TEXT");
    if (!kids.some((c) => isShellChild(node, c, depth))) break;
    const content = kids
      .filter((c) => !isShellChild(node, c, depth) && kidsOf(c).length)
      .sort((a, b) => b.width * b.height - a.width * a.height)[0];
    if (!content) break;
    node = content;
  }
  return node;
}

/** Child offsets inside `node`, from auto-layout. Returns [{x, y, approx}]. */
function placeChildren(node, kids) {
  const dir = node.layoutMode;
  if (dir !== "HORIZONTAL" && dir !== "VERTICAL")
    return kids.map((c) => ({ x: c.x ?? 0, y: c.y ?? 0, approx: c.x == null }));
  const horizontal = dir === "HORIZONTAL";
  const pad = {
    l: node.paddingLeft ?? 0,
    r: node.paddingRight ?? 0,
    t: node.paddingTop ?? 0,
    b: node.paddingBottom ?? 0,
  };
  const innerMain = horizontal
    ? node.width - pad.l - pad.r
    : node.height - pad.t - pad.b;
  const innerCross = horizontal
    ? node.height - pad.t - pad.b
    : node.width - pad.l - pad.r;
  const main = (c) => (horizontal ? c.width : c.height);
  const cross = (c) => (horizontal ? c.height : c.width);
  const total = kids.reduce((s, c) => s + main(c), 0);
  let gap = node.itemSpacing ?? 0;
  const align = node.primaryAxisAlignItems ?? "MIN";
  if (align === "SPACE_BETWEEN" && kids.length > 1)
    gap = (innerMain - total) / (kids.length - 1);
  const used = total + gap * Math.max(kids.length - 1, 0);
  let cursor =
    align === "CENTER"
      ? (innerMain - used) / 2
      : align === "MAX"
        ? innerMain - used
        : 0;
  const crossAlign = node.counterAxisAlignItems ?? "MIN";
  return kids.map((c) => {
    const m = cursor;
    cursor += main(c) + gap;
    const k =
      crossAlign === "CENTER"
        ? (innerCross - cross(c)) / 2
        : crossAlign === "MAX"
          ? innerCross - cross(c)
          : 0;
    return horizontal
      ? { x: pad.l + m, y: pad.t + k }
      : { x: pad.l + k, y: pad.t + m };
  });
}

const KIND = {
  TEXT: "text",
  RECTANGLE: "rect",
  ELLIPSE: "rect",
  INSTANCE: "instance",
  COMPONENT: "instance",
  FRAME: "frame",
  GROUP: "frame",
  SECTION: "frame",
};

/** Normalise one dump into a snapshot. */
export function normalize(
  dump,
  { tokens = loadTokens(), rootName, source } = {},
) {
  const top = dump.component ?? dump;
  const ids = new Map();
  collectIds(top, ids);
  const unmapped = new Map();
  const unbound = new Map();
  const drift = new Map();
  const usedTokens = new Set();
  const note = (map, key, extra) => {
    const hit = map.get(key) ?? { ...extra, count: 0 };
    hit.count += 1;
    map.set(key, hit);
  };

  const reverse = new Map();
  for (const [name, rgb] of Object.entries(tokens.colors))
    if (!reverse.has(rgb)) reverse.set(rgb, name);

  const colorOf = (node, key, entry, report) => {
    const { paint, index } = entry;
    const rgb = rgbString(paint.color);
    const name =
      node.boundVariables?.[key]?.[index]?.name ??
      ids.get(paint.boundVariables?.color?.id);
    if (name) {
      const css = TOKEN_MAP[name];
      if (css && tokens.names.has(css)) {
        usedTokens.add(css);
        if (report && tokens.colors[css] && tokens.colors[css] !== rgb)
          note(drift, name, { css, design: rgb, code: tokens.colors[css] });
        return { token: css, figma: name, rgb };
      }
      if (report) note(unmapped, name, { rgb });
      return { figma: name, rgb, unmapped: true };
    }
    const token = reverse.get(rgb);
    if (token) {
      usedTokens.add(token);
      return { token, rgb, via: "value" };
    }
    if (report) note(unbound, rgb, {});
    return { rgb, unbound: true };
  };

  const topNode = top;
  const rootNode = detectRoot(topNode, rootName);
  let rootOrigin = { x: 0, y: 0 };
  // Absolute origin of every node, filled by a layout pass over the whole dump.
  const origins = new Map();
  (function layout(node, x, y) {
    origins.set(node, { x, y });
    const kids = kidsOf(node);
    const placed = placeChildren(node, kids);
    kids.forEach((child, i) => layout(child, x + placed[i].x, y + placed[i].y));
    kids.forEach((child, i) => {
      if (placed[i].approx) child.__approx = true;
    });
  })(topNode, 0, 0);
  rootOrigin = origins.get(rootNode);

  const build = (node, insideIcon, inPage = false) => {
    const quiet = inPage || PAGE_CONTENT_ROLES.has(node.name);
    const abs = origins.get(node);
    const kind =
      KIND[node.type] ??
      (insideIcon || !(node.children ?? []).length ? "icon" : "frame");
    const out = {
      role: node.name,
      kind,
      box: {
        x: round(abs.x - rootOrigin.x),
        y: round(abs.y - rootOrigin.y),
        w: round(node.width),
        h: round(node.height),
      },
    };
    if (node.__approx) out.approx = true;
    const sizing = {
      h: node.layoutSizingHorizontal,
      v: node.layoutSizingVertical,
    };
    if (sizing.h || sizing.v) out.sizing = sizing;
    if (kind === "icon") return out;
    if (node.layoutMode === "HORIZONTAL" || node.layoutMode === "VERTICAL") {
      out.layout = {
        dir: node.layoutMode === "HORIZONTAL" ? "row" : "column",
        gap: round(node.itemSpacing ?? 0),
        padding: [
          node.paddingTop ?? 0,
          node.paddingRight ?? 0,
          node.paddingBottom ?? 0,
          node.paddingLeft ?? 0,
        ].map(round),
        align: {
          main: node.primaryAxisAlignItems ?? "MIN",
          cross: node.counterAxisAlignItems ?? "MIN",
        },
      };
    }
    if (node.cornerRadius) out.radius = round(node.cornerRadius);
    const stroke = paintsOf(node, "strokes")[0];
    if (stroke)
      out.stroke = {
        w: round(node.strokeWeight ?? 1),
        ...colorOf(node, "strokes", stroke, !quiet),
      };
    if (kind === "text") {
      const fill = paintsOf(node, "fills")[0];
      const ls = node.letterSpacing;
      const lh = node.lineHeight;
      out.text = {
        chars: node.characters ?? "",
        size: node.fontSize,
        weight: node.fontWeight,
        lineHeight: lh?.unit === "PIXELS" ? round(lh.value) : null,
        letterSpacing:
          ls?.unit === "PIXELS"
            ? round(ls.value)
            : ls?.unit === "PERCENT"
              ? round((node.fontSize * ls.value) / 100)
              : 0,
        color: fill ? colorOf(node, "fills", fill, !quiet) : null,
        align: node.textAlignHorizontal ?? "LEFT",
      };
      if (node.textCase && node.textCase !== "ORIGINAL")
        out.text.textCase = node.textCase;
      return out;
    }
    const fill = paintsOf(node, "fills")[0];
    if (fill) out.fill = colorOf(node, "fills", fill, !quiet);
    const children = kidsOf(node).map((c) =>
      build(c, node.type === "VECTOR" || insideIcon, quiet),
    );
    if (children.length) out.children = children;
    return out;
  };

  const tree = build(rootNode, false);
  const stats = { text: 0, box: 0 };
  (function count(n) {
    if (n.kind === "text") stats.text += 1;
    else if ((n.fill || n.stroke) && n !== tree) stats.box += 1;
    (n.children ?? []).forEach(count);
  })(tree);

  return {
    version: 1,
    source: source ?? null,
    frame: topNode.name,
    frameSize: { w: topNode.width, h: topNode.height },
    root: { name: rootNode.name, w: rootNode.width, h: rootNode.height },
    stats,
    unmapped: [...unmapped].map(([figma, v]) => ({ figma, ...v })),
    unbound: [...unbound].map(([rgb, v]) => ({ rgb, count: v.count })),
    drift: [...drift].map(([figma, v]) => ({ figma, ...v })),
    tokensUsed: [...usedTokens].sort(),
    tree,
  };
}

function main(argv) {
  const args = { _: [] };
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--out") args.out = argv[(i += 1)];
    else if (argv[i] === "--root") args.root = argv[(i += 1)];
    else args._.push(argv[i]);
  }
  if (!args._.length) {
    console.error(
      "usage: node scripts/figma-export.mjs <raw.json>... [--out dir] [--root <layer name>]",
    );
    return 2;
  }
  const outDir = path.resolve(
    args.out ?? path.join(repoRoot, "concepts/figma/export/snapshots"),
  );
  fs.mkdirSync(outDir, { recursive: true });
  for (const file of args._) {
    const snapshot = normalize(JSON.parse(fs.readFileSync(file, "utf8")), {
      rootName: args.root,
      source: path.basename(file),
    });
    const target = path.join(outDir, path.basename(file));
    fs.writeFileSync(target, `${JSON.stringify(snapshot, null, 2)}\n`);
    console.log(
      `${path.basename(file)}: root "${snapshot.root.name}" ${snapshot.root.w}x${snapshot.root.h}, ${snapshot.stats.text} text, ${snapshot.stats.box} box -> ${target}`,
    );
    for (const u of snapshot.unmapped)
      console.log(`  UNMAPPED ${u.figma} (${u.rgb}) x${u.count}`);
    for (const u of snapshot.unbound)
      console.log(`  UNBOUND ${u.rgb} x${u.count}`);
    for (const d of snapshot.drift)
      console.log(
        `  DRIFT ${d.figma}: figma ${d.design} vs ${d.css} ${d.code}`,
      );
  }
  return 0;
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
)
  process.exitCode = main(process.argv.slice(2));
