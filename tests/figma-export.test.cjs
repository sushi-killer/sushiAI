const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const load = (name) => import(`../scripts/${name}.mjs`);

const TOKENS_CSS = `:root {
  --bg-app: #0b0b0b;
  --bg-raised: #171717;
  --line: #303030;
  --text: #e7e8e0;
  --text-muted: #8f948d;
  --alias: var(--text);
}`;

const rgb = (hex) => {
  const n = parseInt(hex.slice(1), 16);
  return {
    r: ((n >> 16) & 255) / 255,
    g: ((n >> 8) & 255) / 255,
    b: (n & 255) / 255,
  };
};
const paint = (hex, id) => ({
  type: "SOLID",
  visible: true,
  opacity: 1,
  color: rgb(hex),
  boundVariables: id ? { color: { id } } : {},
});
const bound = (key, name, id) => ({ [key]: [{ id, name, collection: "t" }] });

const text = (name, chars, props = {}) => ({
  name,
  type: "TEXT",
  characters: chars,
  fontSize: 13,
  fontWeight: 400,
  lineHeight: { unit: "AUTO" },
  letterSpacing: { unit: "PERCENT", value: 0 },
  width: 60,
  height: 16,
  fills: [paint("#e7e8e0", "V:text")],
  boundVariables: bound("fills", "text/default", "V:text"),
  ...props,
});

// Window 400x300: title bar, sidebar, then the canvas with one padded page.
const dump = () => ({
  component: {
    name: "F1 synthetic",
    type: "FRAME",
    width: 400,
    height: 300,
    layoutMode: "VERTICAL",
    children: [
      {
        name: "Shell/Titlebar",
        type: "INSTANCE",
        width: 400,
        height: 40,
        children: [],
      },
      {
        name: "body",
        type: "FRAME",
        layoutMode: "HORIZONTAL",
        width: 400,
        height: 260,
        children: [
          {
            name: "sidebar",
            type: "FRAME",
            width: 100,
            height: 260,
            layoutMode: "VERTICAL",
            children: [text("nav", "Nav")],
          },
          {
            name: "canvas",
            type: "FRAME",
            width: 300,
            height: 260,
            layoutMode: "VERTICAL",
            paddingLeft: 10,
            paddingRight: 10,
            paddingTop: 20,
            paddingBottom: 10,
            itemSpacing: 6,
            primaryAxisAlignItems: "MIN",
            counterAxisAlignItems: "MIN",
            fills: [paint("#0b0b0b", "V:app")],
            boundVariables: bound("fills", "bg/app", "V:app"),
            children: [
              text("title", "Title", {
                fontSize: 20,
                fontWeight: 600,
                width: 50,
                height: 24,
              }),
              {
                name: "card",
                type: "FRAME",
                width: 200,
                height: 40,
                layoutMode: "HORIZONTAL",
                paddingLeft: 12,
                paddingTop: 8,
                counterAxisAlignItems: "CENTER",
                cornerRadius: 8,
                fills: [paint("#171717", "V:raised")],
                strokes: [paint("#303030", "V:line")],
                strokeWeight: 1,
                boundVariables: {
                  ...bound("fills", "bg/raised", "V:raised"),
                  ...bound("strokes", "line/default", "V:line"),
                },
                children: [
                  text("label", "Label", {
                    width: 40,
                    fills: [paint("#123456", "V:icon")],
                    boundVariables: bound("fills", "accent/claude", "V:icon"),
                  }),
                ],
              },
              text("hidden", "Hidden", { visible: false }),
              text("raw", "Raw", {
                fills: [paint("#8f948d")],
                boundVariables: {},
              }),
            ],
          },
        ],
      },
    ],
  },
});

const tokensOf = async () =>
  (await load("figma-export")).parseTokens(TOKENS_CSS);

test("parseTokens resolves hex and var() aliases", async () => {
  const tokens = await tokensOf();
  assert.equal(tokens.colors["--bg-app"], "rgb(11, 11, 11)");
  assert.equal(tokens.colors["--alias"], tokens.colors["--text"]);
});

test("the app shell is stripped and boxes are relative to the content root", async () => {
  const { normalize } = await load("figma-export");
  const snap = normalize(dump(), { tokens: await tokensOf() });
  assert.equal(snap.root.name, "canvas");
  assert.deepEqual(snap.tree.box, { x: 0, y: 0, w: 300, h: 260 });
  const [title, card] = snap.tree.children;
  assert.deepEqual(title.box, { x: 10, y: 20, w: 50, h: 24 });
  assert.deepEqual(card.box, { x: 10, y: 50, w: 200, h: 40 });
  assert.deepEqual(card.children[0].box, { x: 22, y: 66, w: 40, h: 16 });
  assert.equal(snap.stats.text, 3);
});

test("--root picks a layer by name and rejects a missing one", async () => {
  const { normalize } = await load("figma-export");
  const tokens = await tokensOf();
  const snap = normalize(dump(), { tokens, rootName: "card" });
  assert.equal(snap.tree.role, "card");
  assert.equal(snap.tree.box.x, 0);
  assert.throws(
    () => normalize(dump(), { tokens, rootName: "nope" }),
    /no layer named/,
  );
});

test("variables map to tokens; unmapped and unbound colours are reported", async () => {
  const { normalize } = await load("figma-export");
  const snap = normalize(dump(), { tokens: await tokensOf() });
  const [title, card] = snap.tree.children;
  assert.equal(snap.tree.fill.token, "--bg-app");
  assert.equal(title.text.color.token, "--text");
  assert.equal(card.fill.token, "--bg-raised");
  assert.equal(card.stroke.token, "--line");
  assert.equal(card.stroke.w, 1);
  assert.equal(card.radius, 8);
  assert.deepEqual(
    snap.unmapped.map((u) => u.figma),
    ["accent/claude"],
  );
  // The unbound grey equals --text-muted, so it resolves by value.
  const raw = snap.tree.children[2];
  assert.equal(raw.text.color.token, "--text-muted");
  assert.equal(raw.text.color.via, "value");
  assert.equal(snap.tree.children.length, 3, "hidden layers are dropped");
});

test("layout metrics are kept", async () => {
  const { normalize } = await load("figma-export");
  const snap = normalize(dump(), { tokens: await tokensOf() });
  assert.deepEqual(snap.tree.layout, {
    dir: "column",
    gap: 6,
    padding: [20, 10, 10, 10],
    align: { main: "MIN", cross: "MIN" },
  });
});

test("codegen is deterministic, tokens only and flags unmapped colours", async () => {
  const { normalize } = await load("figma-export");
  const { generate } = await load("figma-codegen");
  const snap = normalize(dump(), { tokens: await tokensOf() });
  const a = generate(snap);
  const b = generate(snap);
  assert.equal(a.css, b.css);
  assert.match(a.css, /\.fx-card \{/);
  assert.match(a.css, /gap: 6px;/);
  assert.match(a.css, /padding: 20px 10px 10px 10px;/);
  assert.match(a.css, /border-radius: 8px;/);
  assert.match(a.css, /background: var\(--bg-raised\);/);
  assert.match(a.css, /UNMAPPED color: accent\/claude/);
  assert.doesNotMatch(a.css, /#[0-9a-f]{3,8}\b|rgb\(/i);
  assert.doesNotMatch(a.css, /Label|Title/);
  assert.ok(a.unmapped.some((line) => line.includes("accent/claude")));
  assert.ok(a.map.every((m) => m.class.startsWith("fx-")));
});

const measureOf = (snap) => {
  const texts = [];
  const boxes = [];
  const color = (c) =>
    c?.token ? { rgb: c.rgb, tokens: [c.token] } : { rgb: c?.rgb, tokens: [] };
  (function walk(n) {
    if (n.kind === "text")
      texts.push({
        text: n.text.chars,
        ...n.box,
        fontSize: n.text.size,
        fontWeight: n.text.weight,
        lineHeight: null,
        color: color(n.text.color),
      });
    else if ((n.fill || n.stroke) && n !== snap.tree)
      boxes.push({
        ...n.box,
        radius: n.radius ?? 0,
        bg: n.fill ? color(n.fill) : null,
        border: n.stroke ? color(n.stroke) : null,
      });
    (n.children ?? []).forEach(walk);
  })(snap.tree);
  return { version: 1, root: { w: 300, h: 260 }, texts, boxes };
};

test("parity passes on an identical measurement", async () => {
  const { normalize } = await load("figma-export");
  const { compare } = await load("figma-parity");
  const snap = normalize(dump(), { tokens: await tokensOf() });
  const result = compare(snap, measureOf(snap));
  assert.equal(result.summary.fail, 0);
  assert.ok(result.summary.checks > 10);
  assert.equal(result.extra.length, 0);
});

test("parity fails on size, weight, colour, missing text and missing box", async () => {
  const { normalize } = await load("figma-export");
  const { compare } = await load("figma-parity");
  const snap = normalize(dump(), { tokens: await tokensOf() });
  const m = measureOf(snap);
  m.texts.find((t) => t.text === "Title").fontSize = 18;
  m.texts.find((t) => t.text === "Title").fontWeight = 500;
  m.texts.find((t) => t.text === "Raw").color = {
    rgb: "rgb(1, 2, 3)",
    tokens: ["--text"],
  };
  m.texts = m.texts.filter((t) => t.text !== "Label");
  m.boxes = m.boxes.filter((b) => b.radius !== 8);
  const result = compare(snap, m);
  const failing = result.rows.filter((r) => r.status === "FAIL");
  const has = (anchor, field) =>
    failing.some((r) => r.anchor.startsWith(anchor) && r.field === field);
  assert.ok(has("Title", "fontSize"));
  assert.ok(has("Title", "fontWeight"));
  assert.ok(has("Raw", "color"));
  assert.ok(has("Label", "text"));
  assert.ok(has("card", "box"));
});

test("tolerance edges: 2px position and 3% width pass, beyond fails", async () => {
  const { normalize } = await load("figma-export");
  const { compare } = await load("figma-parity");
  const snap = normalize(dump(), { tokens: await tokensOf() });
  const check = (patch) => {
    const m = measureOf(snap);
    Object.assign(
      m.texts.find((t) => t.text === "Title"),
      patch,
    );
    return compare(snap, m).rows.filter(
      (r) => r.status === "FAIL" && r.anchor === "Title",
    );
  };
  assert.equal(check({ x: 12, y: 18 }).length, 0);
  assert.deepEqual(
    check({ x: 12.01 }).map((r) => r.field),
    ["x"],
  );
  assert.equal(check({ w: 51.5 }).length, 0);
  assert.deepEqual(
    check({ w: 51.6 }).map((r) => r.field),
    ["w"],
  );
});

test("an app text node without a Figma counterpart is EXTRA, not a failure", async () => {
  const { normalize } = await load("figma-export");
  const { compare } = await load("figma-parity");
  const snap = normalize(dump(), { tokens: await tokensOf() });
  const m = measureOf(snap);
  m.texts.push({
    text: "Surplus",
    x: 0,
    y: 0,
    w: 10,
    h: 10,
    fontSize: 12,
    fontWeight: 400,
  });
  const result = compare(snap, m);
  assert.equal(result.summary.fail, 0);
  assert.deepEqual(
    result.extra.map((e) => e.anchor),
    ["Surplus"],
  );
});

test("the CLIs write snapshot and exit 1 on a parity failure", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "figma-export-"));
  try {
    const raw = path.join(dir, "S1.json");
    fs.writeFileSync(raw, JSON.stringify(dump()));
    const run = (script, ...args) =>
      spawnSync(
        process.execPath,
        [path.join(__dirname, "..", "scripts", script), ...args],
        { encoding: "utf8" },
      );
    const exp = run("figma-export.mjs", raw, "--out", dir);
    assert.equal(exp.status, 0, exp.stderr);
    assert.match(exp.stdout, /UNMAPPED accent\/claude/);
    const out = JSON.parse(fs.readFileSync(path.join(dir, "S1.json"), "utf8"));
    assert.equal(out.version, 1);
    const measure = path.join(dir, "m.json");
    fs.writeFileSync(measure, JSON.stringify({ texts: [], boxes: [] }));
    const par = run(
      "figma-parity.mjs",
      path.join(dir, "S1.json"),
      measure,
      "--json",
    );
    assert.equal(par.status, 1);
    assert.ok(JSON.parse(par.stdout).summary.fail > 0);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("measureScript is an evaluable function expression", async () => {
  const { measureScript, measureExpression } = await load("figma-parity");
  assert.equal(typeof new Function(`return ${measureScript}`)(), "function");
  assert.match(measureExpression("#r"), /\("#r"\)$/);
});
