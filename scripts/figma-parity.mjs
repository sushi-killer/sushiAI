#!/usr/bin/env node
// Compare a Figma snapshot with a measurement of the running app.
//
//   node scripts/figma-parity.mjs <snapshot.json> <measure.json> [--json]
//        [--pos 2] [--text-width 3] [--frame <label>]
//
// The measurement comes from `measureScript` below, run in the page (see
// .agents/skills/ui-evidence/scripts/measure.mjs). Both sides are relative to
// the same content root.
//
// Matching is text-anchored. A Figma text layer pairs with the app text node
// of the same string (after the layer's text case), nearest first, one to one.
// Boxes (filled or bordered layers) pair by geometry: the nearest unused app
// box within `--pos` + 4px on every edge. Compared: x/y/w/h within `--pos`
// px (text width within `--text-width` percent, at least 1px), fontSize and
// fontWeight exactly, colour by token (the app side lists every token that
// resolves to its colour; without a Figma token the rgb must be equal),
// lineHeight when both are pixel values (0.5px), radius within 1px.
// A Figma anchor with no app counterpart is a FAIL; an app anchor with no
// Figma counterpart is listed as EXTRA and does not fail the run.
// Exit code 1 on any FAIL, 2 on bad usage.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

/** Runs in the page. Takes a root selector; returns the measurement. */
function measure(rootSelector) {
  const root = document.querySelector(rootSelector || "body");
  if (!root) throw new Error(`measure: no element for "${rootSelector}"`);
  const origin = root.getBoundingClientRect();
  const r2 = (n) => Math.round(n * 100) / 100;
  const probe = document.createElement("span");
  probe.style.display = "none";
  document.body.append(probe);
  const tokens = [];
  const seen = new Set();
  for (const sheet of document.styleSheets) {
    let rules;
    try {
      rules = sheet.cssRules;
    } catch {
      continue;
    }
    for (const rule of rules ?? [])
      if (rule.style && /^:root$/.test(rule.selectorText ?? ""))
        for (const prop of rule.style)
          if (prop.startsWith("--") && !seen.has(prop)) {
            seen.add(prop);
            probe.style.color = "";
            probe.style.color = `var(${prop})`;
            const value = getComputedStyle(probe).color;
            if (value && value.startsWith("rgb")) tokens.push([prop, value]);
          }
  }
  probe.remove();
  const norm = (c) => {
    const m = /^rgba?\(([^)]+)\)$/.exec(c);
    if (!m) return null;
    const p = m[1]
      .split(/[ ,/]+/)
      .filter(Boolean)
      .map(Number);
    if (p.length > 3 && p[3] === 0) return null;
    return `rgb(${p[0]}, ${p[1]}, ${p[2]})`;
  };
  const colorInfo = (c) => {
    const rgb = norm(c);
    if (!rgb) return null;
    return {
      rgb,
      tokens: tokens.filter(([, v]) => norm(v) === rgb).map(([k]) => k),
    };
  };
  const visible = (el) => {
    for (let n = el; n && n !== document.documentElement; n = n.parentElement) {
      const s = getComputedStyle(n);
      if (
        s.display === "none" ||
        s.visibility === "hidden" ||
        s.opacity === "0"
      )
        return false;
    }
    return true;
  };
  const inside = (rect) =>
    rect.width > 0 &&
    rect.height > 0 &&
    rect.right > origin.left &&
    rect.left < origin.right &&
    rect.bottom > origin.top &&
    rect.top < origin.bottom;
  const rel = (rect) => ({
    x: r2(rect.left - origin.left),
    y: r2(rect.top - origin.top),
    w: r2(rect.width),
    h: r2(rect.height),
  });
  const textOf = (raw, style) => {
    let t = raw.replace(/\s+/g, " ").trim();
    if (style.textTransform === "uppercase") t = t.toUpperCase();
    if (style.textTransform === "lowercase") t = t.toLowerCase();
    return t;
  };
  const font = (style) => ({
    fontSize: parseFloat(style.fontSize),
    fontWeight: Number(style.fontWeight),
    lineHeight:
      style.lineHeight === "normal" ? null : r2(parseFloat(style.lineHeight)),
    color: colorInfo(style.color),
  });
  const texts = [];
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const el = node.parentElement;
    if (!el || ["SCRIPT", "STYLE", "TEXTAREA"].includes(el.tagName)) continue;
    const style = getComputedStyle(el);
    const text = textOf(node.textContent, style);
    if (!text || !visible(el)) continue;
    const range = document.createRange();
    range.selectNodeContents(node);
    const rect = range.getBoundingClientRect();
    if (!inside(rect)) continue;
    texts.push({ text, ...rel(rect), ...font(style) });
  }
  for (const el of root.querySelectorAll("input, textarea")) {
    const text = (el.value || el.placeholder || "").trim();
    if (!text || !visible(el)) continue;
    const rect = el.getBoundingClientRect();
    if (!inside(rect)) continue;
    const style = getComputedStyle(el);
    const left =
      parseFloat(style.paddingLeft) + parseFloat(style.borderLeftWidth);
    const top = parseFloat(style.paddingTop) + parseFloat(style.borderTopWidth);
    const box = rel(rect);
    texts.push({
      text,
      ...box,
      x: r2(box.x + left),
      y: r2(box.y + top),
      w: r2(box.w - 2 * left),
      h: r2(box.h - 2 * top),
      ...font(style),
    });
  }
  const boxes = [];
  for (const el of root.querySelectorAll("*")) {
    const style = getComputedStyle(el);
    const rect = el.getBoundingClientRect();
    if (!inside(rect) || !visible(el)) continue;
    const bg = colorInfo(style.backgroundColor);
    const bw = parseFloat(style.borderTopWidth) || 0;
    const border =
      bw > 0 && style.borderTopStyle !== "none"
        ? colorInfo(style.borderTopColor)
        : null;
    if (!bg && !border) continue;
    boxes.push({
      ...rel(rect),
      radius: r2(parseFloat(style.borderTopLeftRadius) || 0),
      bg,
      border,
      borderWidth: border ? bw : 0,
    });
  }
  return {
    version: 1,
    root: { w: r2(origin.width), h: r2(origin.height) },
    texts,
    boxes,
  };
}

/** Function source, for `page.evaluate(measureExpression(selector))`. */
export const measureScript = `(${measure.toString()})`;
export const measureExpression = (rootSelector = "body") =>
  `${measureScript}(${JSON.stringify(rootSelector)})`;
export { measure };

const DEFAULTS = { pos: 2, textWidthPct: 3 };

/** Flatten a snapshot tree into text and box anchors. */
export function anchorsOf(snapshot) {
  const texts = [];
  const boxes = [];
  (function walk(node, frame, isRoot) {
    const label = node.kind === "text" ? frame : node.role;
    if (node.kind === "text") {
      let chars = node.text.chars;
      if (node.text.textCase === "UPPER") chars = chars.toUpperCase();
      if (node.text.textCase === "LOWER") chars = chars.toLowerCase();
      texts.push({ text: chars.replace(/\s+/g, " ").trim(), frame, node });
    } else if (!isRoot && node.kind !== "icon" && (node.fill || node.stroke)) {
      boxes.push({ frame: label, node });
    }
    const next =
      node.kind === "text" || node.kind === "icon" ? frame : node.role;
    (node.children ?? []).forEach((c) => walk(c, next, false));
  })(snapshot.tree, snapshot.tree.role, true);
  return { texts, boxes };
}

const colorMatches = (fig, app) => {
  if (!fig) return true;
  if (!app) return false;
  if (fig.token) return app.tokens.includes(fig.token);
  return app.rgb === fig.rgb;
};
const describeApp = (c) => (c ? (c.tokens[0] ?? c.rgb) : "none");
const describeFig = (c) => (c ? (c.token ?? c.figma ?? c.rgb) : "none");

/** Compare a snapshot with a measurement. Returns {rows, extra, summary}. */
export function compare(snapshot, measurement, options = {}) {
  const tol = { ...DEFAULTS, ...options };
  const rows = [];
  const extra = [];
  const add = (frame, anchor, field, figma, app, delta, pass) =>
    rows.push({
      frame,
      anchor,
      field,
      figma,
      app,
      delta,
      status: pass ? "PASS" : "FAIL",
    });
  const num = (frame, anchor, field, f, a, limit) => {
    const delta = Math.round((a - f) * 100) / 100;
    add(frame, anchor, field, f, a, delta, Math.abs(delta) <= limit + 1e-9);
  };
  const exact = (frame, anchor, field, f, a) =>
    add(frame, anchor, field, f, a, f === a ? 0 : "!=", f === a);

  const { texts, boxes } = anchorsOf(snapshot);
  const usedText = new Set();
  for (const fig of texts) {
    const b = fig.node.box;
    let best = -1;
    let bestDist = Infinity;
    measurement.texts.forEach((app, i) => {
      if (usedText.has(i) || app.text !== fig.text) return;
      const dist = Math.abs(app.x - b.x) + Math.abs(app.y - b.y);
      if (dist < bestDist) {
        bestDist = dist;
        best = i;
      }
    });
    if (best < 0) {
      add(fig.frame, fig.text, "text", fig.text, null, "missing", false);
      continue;
    }
    usedText.add(best);
    const app = measurement.texts[best];
    const t = fig.node.text;
    const f = fig.frame;
    const a = fig.text;
    num(f, a, "x", b.x, app.x, tol.pos);
    num(f, a, "y", b.y, app.y, tol.pos);
    num(f, a, "w", b.w, app.w, Math.max((b.w * tol.textWidthPct) / 100, 1));
    num(f, a, "h", b.h, app.h, tol.pos + 1);
    exact(f, a, "fontSize", t.size, app.fontSize);
    exact(f, a, "fontWeight", t.weight, app.fontWeight);
    if (t.lineHeight != null && app.lineHeight != null)
      num(f, a, "lineHeight", t.lineHeight, app.lineHeight, 0.5);
    if (t.color) {
      const ok = colorMatches(t.color, app.color);
      add(
        f,
        a,
        "color",
        describeFig(t.color),
        describeApp(app.color),
        ok ? 0 : "!=",
        ok,
      );
    }
  }
  measurement.texts.forEach((app, i) => {
    if (!usedText.has(i))
      extra.push({ kind: "text", anchor: app.text, x: app.x, y: app.y });
  });

  const usedBox = new Set();
  const reach = tol.pos + 4;
  for (const fig of boxes) {
    const b = fig.node.box;
    let best = -1;
    let bestDist = Infinity;
    measurement.boxes.forEach((app, i) => {
      if (usedBox.has(i)) return;
      const d = [app.x - b.x, app.y - b.y, app.w - b.w, app.h - b.h].map(
        Math.abs,
      );
      if (Math.max(...d) > reach) return;
      const dist = d.reduce((s, v) => s + v, 0);
      if (dist < bestDist) {
        bestDist = dist;
        best = i;
      }
    });
    const anchor = `${fig.frame} @${b.x},${b.y} ${b.w}x${b.h}`;
    if (best < 0) {
      add(fig.frame, anchor, "box", `${b.w}x${b.h}`, null, "missing", false);
      continue;
    }
    usedBox.add(best);
    const app = measurement.boxes[best];
    const f = fig.frame;
    num(f, anchor, "x", b.x, app.x, tol.pos);
    num(f, anchor, "y", b.y, app.y, tol.pos);
    num(f, anchor, "w", b.w, app.w, tol.pos);
    num(f, anchor, "h", b.h, app.h, tol.pos);
    if (fig.node.radius != null)
      num(f, anchor, "radius", fig.node.radius, app.radius, 1);
    if (fig.node.fill) {
      const ok = colorMatches(fig.node.fill, app.bg);
      add(
        f,
        anchor,
        "bg",
        describeFig(fig.node.fill),
        describeApp(app.bg),
        ok ? 0 : "!=",
        ok,
      );
    }
    if (fig.node.stroke) {
      const ok = colorMatches(fig.node.stroke, app.border);
      add(
        f,
        anchor,
        "border",
        describeFig(fig.node.stroke),
        describeApp(app.border),
        ok ? 0 : "!=",
        ok,
      );
    }
  }
  measurement.boxes.forEach((app, i) => {
    if (!usedBox.has(i))
      extra.push({
        kind: "box",
        anchor: `@${app.x},${app.y} ${app.w}x${app.h}`,
        x: app.x,
        y: app.y,
      });
  });

  const fails = rows.filter((r) => r.status === "FAIL").length;
  return {
    rows,
    extra,
    summary: {
      anchors: texts.length + boxes.length,
      figmaTexts: texts.length,
      figmaBoxes: boxes.length,
      checks: rows.length,
      fail: fails,
      pass: rows.length - fails,
      extra: extra.length,
    },
  };
}

function formatTable(result) {
  const head = ["frame", "anchor", "field", "figma", "app", "delta", "result"];
  const cut = (v, n) => {
    const s = String(v ?? "-");
    return s.length > n ? `${s.slice(0, n - 1)}~` : s;
  };
  const lines = [head.join(" | ")];
  for (const r of result.rows)
    lines.push(
      [
        cut(r.frame, 24),
        cut(r.anchor, 40),
        r.field,
        cut(r.figma, 24),
        cut(r.app, 24),
        r.delta,
        r.status,
      ].join(" | "),
    );
  for (const e of result.extra)
    lines.push(
      `- | ${cut(e.anchor, 40)} | ${e.kind} | - | @${e.x},${e.y} | - | EXTRA`,
    );
  const s = result.summary;
  lines.push(
    `${s.fail ? "FAIL" : "PASS"}: ${s.pass}/${s.checks} checks, ${s.fail} failing, ${s.extra} extra app anchors`,
  );
  return lines.join("\n");
}

function main(argv) {
  const args = {
    _: [],
    pos: DEFAULTS.pos,
    textWidthPct: DEFAULTS.textWidthPct,
  };
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--json") args.json = true;
    else if (argv[i] === "--pos") args.pos = Number(argv[(i += 1)]);
    else if (argv[i] === "--text-width")
      args.textWidthPct = Number(argv[(i += 1)]);
    else if (argv[i] === "--frame") args.frame = argv[(i += 1)];
    else args._.push(argv[i]);
  }
  if (
    args._.length !== 2 ||
    Number.isNaN(args.pos) ||
    Number.isNaN(args.textWidthPct)
  ) {
    console.error(
      "usage: node scripts/figma-parity.mjs <snapshot.json> <measure.json> [--json] [--pos 2] [--text-width 3] [--frame <label>]",
    );
    return 2;
  }
  const snapshot = JSON.parse(fs.readFileSync(args._[0], "utf8"));
  const measurement = JSON.parse(fs.readFileSync(args._[1], "utf8"));
  const result = compare(snapshot, measurement, args);
  if (args.frame) {
    result.rows = result.rows.filter((r) => r.frame === args.frame);
    result.summary.checks = result.rows.length;
    result.summary.fail = result.rows.filter((r) => r.status === "FAIL").length;
    result.summary.pass = result.summary.checks - result.summary.fail;
  }
  console.log(
    args.json ? JSON.stringify(result, null, 2) : formatTable(result),
  );
  return result.summary.fail ? 1 : 0;
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
)
  process.exitCode = main(process.argv.slice(2));
