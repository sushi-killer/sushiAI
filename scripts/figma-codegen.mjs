#!/usr/bin/env node
// Emit exact layout metrics from a snapshot as CSS classes, tokens only.
//
//   node scripts/figma-codegen.mjs <snapshot.json> [--out file.css] [--map file.json] [--prefix fx]
//
// A helper for the UI author, never a runtime dependency: apply the classes
// (or copy the values) to real components, then delete what is not needed.
// One class per layer role: `.fx-<slug>`; the same role with different
// metrics gets `-2`, `-3` in order of first appearance. A text layer is named
// after its nearest frame (`.fx-<frame>-t<n>`) so no copy ends up in a class.
// Output is deterministic. A colour is emitted only as `var(--token)`: a
// colour without a token becomes a comment and an UNMAPPED line on stderr.
// Defaults: concepts/figma/export/generated/<snapshot>.generated.css and
// `.map.json` (gitignored); pass --out src/styles/components/<area>.generated.css
// to place it in the tree.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "..");

const slugOf = (text) =>
  String(text)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40)
    .replace(/-+$/g, "") || "layer";

const px = (n) => `${Math.round(n * 100) / 100}px`;
const FLEX_ALIGN = {
  MIN: "flex-start",
  CENTER: "center",
  MAX: "flex-end",
  SPACE_BETWEEN: "space-between",
};

function colorDecl(prop, color, unmapped, role) {
  if (color?.token) return [`${prop}: var(${color.token});`];
  const why = color?.figma ?? color?.rgb ?? "unknown";
  unmapped.push(`${role}: ${prop} ${why}`);
  return [`/* UNMAPPED ${prop}: ${why} */`];
}

function declsFor(node, parent, unmapped) {
  const d = [];
  const sizing = node.sizing ?? {};
  const mainRow = parent?.layout?.dir === "row";
  const fillMain = mainRow ? sizing.h === "FILL" : sizing.v === "FILL";
  const fillCross = mainRow ? sizing.v === "FILL" : sizing.h === "FILL";
  if (node.kind === "text") {
    const t = node.text;
    if (sizing.h !== "FILL" && sizing.h !== "HUG")
      d.push(`width: ${px(node.box.w)};`);
    d.push(`font-size: ${px(t.size)};`, `font-weight: ${t.weight};`);
    if (t.lineHeight != null) d.push(`line-height: ${px(t.lineHeight)};`);
    if (t.letterSpacing) d.push(`letter-spacing: ${px(t.letterSpacing)};`);
    if (t.textCase === "UPPER") d.push("text-transform: uppercase;");
    if (t.align && t.align !== "LEFT")
      d.push(`text-align: ${t.align.toLowerCase()};`);
    if (t.color) d.push(...colorDecl("color", t.color, unmapped, node.role));
    return d;
  }
  if (sizing.h === "FIXED" && !(parent && mainRow && fillMain))
    d.push(`width: ${px(node.box.w)};`);
  if (sizing.v === "FIXED" && !(parent && !mainRow && fillMain))
    d.push(`height: ${px(node.box.h)};`);
  if (parent?.layout && fillMain) d.push("flex: 1 1 0;");
  if (parent?.layout && fillCross) d.push("align-self: stretch;");
  if (node.layout) {
    const { dir, gap, padding, align } = node.layout;
    d.push("display: flex;", `flex-direction: ${dir};`);
    if (gap) d.push(`gap: ${px(gap)};`);
    if (padding.some(Boolean)) d.push(`padding: ${padding.map(px).join(" ")};`);
    d.push(`align-items: ${FLEX_ALIGN[align.cross] ?? "flex-start"};`);
    if (align.main !== "MIN")
      d.push(`justify-content: ${FLEX_ALIGN[align.main] ?? "flex-start"};`);
  }
  if (node.radius) d.push(`border-radius: ${px(node.radius)};`);
  if (node.stroke) {
    d.push(`border-width: ${px(node.stroke.w)};`, "border-style: solid;");
    d.push(...colorDecl("border-color", node.stroke, unmapped, node.role));
  }
  if (node.fill)
    d.push(...colorDecl("background", node.fill, unmapped, node.role));
  return d;
}

/** Snapshot -> {css, map, unmapped}. Pure and deterministic. */
export function generate(snapshot, { prefix = "fx" } = {}) {
  const unmapped = [];
  const variants = new Map(); // base class -> [{key, name}]
  const rules = [];
  const map = [];
  const textCount = new Map();

  const classFor = (base, key) => {
    const list = variants.get(base) ?? [];
    variants.set(base, list);
    let hit = list.find((v) => v.key === key);
    if (!hit) {
      hit = {
        key,
        name: list.length ? `${base}-${list.length + 1}` : base,
        fresh: true,
      };
      list.push(hit);
    }
    return hit;
  };

  (function walk(node, parent, frameSlug, trail) {
    if (node.kind === "icon") return;
    let base;
    let slug = frameSlug;
    if (node.kind === "text") {
      const n = (textCount.get(frameSlug) ?? 0) + 1;
      textCount.set(frameSlug, n);
      base = `${prefix}-${frameSlug}-t${n}`;
    } else {
      slug = slugOf(node.role);
      base = `${prefix}-${slug}`;
    }
    const decls = declsFor(node, parent, unmapped);
    if (decls.length) {
      const hit = classFor(base, decls.join("\n"));
      if (hit.fresh) {
        hit.fresh = false;
        rules.push(
          `/* ${trail.concat(node.kind === "text" ? "text" : node.role).join(" > ")} */\n.${hit.name} {\n${decls.map((x) => `  ${x}`).join("\n")}\n}`,
        );
      }
      map.push({
        role: node.kind === "text" ? `${frameSlug}/text` : node.role,
        kind: node.kind,
        class: hit.name,
        path: trail.join(" > "),
      });
    }
    const nextTrail = node.kind === "text" ? trail : trail.concat(node.role);
    (node.children ?? []).forEach((c) => walk(c, node, slug, nextTrail));
  })(snapshot.tree, null, "root", []);

  const header =
    "/* Generated by scripts/figma-codegen.mjs from a Figma snapshot. Do not edit; apply to components by hand. Tokens only. */\n";
  return { css: `${header}\n${rules.join("\n\n")}\n`, map, unmapped };
}

function main(argv) {
  const args = { _: [] };
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--out") args.out = argv[(i += 1)];
    else if (argv[i] === "--map") args.map = argv[(i += 1)];
    else if (argv[i] === "--prefix") args.prefix = argv[(i += 1)];
    else args._.push(argv[i]);
  }
  if (args._.length !== 1) {
    console.error(
      "usage: node scripts/figma-codegen.mjs <snapshot.json> [--out file.css] [--map file.json] [--prefix fx]",
    );
    return 2;
  }
  const snapshot = JSON.parse(fs.readFileSync(args._[0], "utf8"));
  const { css, map, unmapped } = generate(snapshot, { prefix: args.prefix });
  const stem = path.basename(args._[0], ".json");
  const out = path.resolve(
    args.out ??
      path.join(
        repoRoot,
        "concepts/figma/export/generated",
        `${stem}.generated.css`,
      ),
  );
  const mapOut = path.resolve(args.map ?? out.replace(/\.css$/, ".map.json"));
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.mkdirSync(path.dirname(mapOut), { recursive: true });
  fs.writeFileSync(out, css);
  fs.writeFileSync(mapOut, `${JSON.stringify(map, null, 2)}\n`);
  console.log(`${out} (${map.length} classes), ${mapOut}`);
  for (const line of unmapped) console.error(`UNMAPPED ${line}`);
  return 0;
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
)
  process.exitCode = main(process.argv.slice(2));
