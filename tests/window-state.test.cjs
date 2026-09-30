const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const {
  DEFAULT_BOUNDS,
  loadWindowState,
  saveWindowState,
  clampBounds,
} = require("../electron/window-state.cjs");

const area1 = { x: 0, y: 25, width: 1920, height: 1055 };
const d1 = {
  id: 1,
  bounds: { x: 0, y: 0, width: 1920, height: 1080 },
  workArea: area1,
};
const d2 = {
  id: 2,
  bounds: { x: 1920, y: 0, width: 1280, height: 720 },
  workArea: { x: 1920, y: 0, width: 1280, height: 720 },
};
const state = (bounds, displayId = 1) => ({
  bounds,
  displayId,
  isMaximized: false,
  isFullScreen: false,
});
const inside = (b, a) =>
  b.x >= a.x &&
  b.y >= a.y &&
  b.x + b.width <= a.x + a.width &&
  b.y + b.height <= a.y + a.height;
const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), "window-state-"));

test("default bounds", () => {
  assert.deepEqual(DEFAULT_BOUNDS, { width: 1380, height: 880 });
});

test("bounds inside a work area are unchanged", () => {
  const b = { x: 100, y: 100, width: 1000, height: 700 };
  assert.deepEqual(clampBounds(state(b), [d1, d2], d1), b);
});

test("a window with 30% off-screen keeps its position", () => {
  const b = { x: 1920 + 1280 - 700, y: 100, width: 1000, height: 600 };
  assert.deepEqual(clampBounds(state(b, 2), [d1, d2], d1), b);
});

test("a missing display moves the window onto primary", () => {
  const b = { x: 5000, y: 300, width: 1000, height: 700 };
  const out = clampBounds(state(b, 99), [d1], d1);
  assert.ok(inside(out, area1));
  assert.equal(out.width, 1000);
});

test("a mostly off-screen window is pulled inside", () => {
  const b = { x: -800, y: 100, width: 1000, height: 700 };
  const out = clampBounds(state(b), [d1], d1);
  assert.ok(inside(out, area1));
});

test("oversized bounds shrink to the work area", () => {
  const b = { x: 0, y: 25, width: 4000, height: 3000 };
  const out = clampBounds(state(b), [d1], d1);
  assert.equal(out.width, area1.width);
  assert.equal(out.height, area1.height);
});

test("bounds never shrink below 600x440", () => {
  const small = {
    id: 3,
    bounds: { x: 0, y: 0, width: 500, height: 400 },
    workArea: { x: 0, y: 0, width: 500, height: 400 },
  };
  const out = clampBounds(
    state({ x: 0, y: 0, width: 900, height: 700 }, 3),
    [small],
    small,
  );
  assert.equal(out.width, 600);
  assert.equal(out.height, 440);
});

test("state round-trips and leaves no tmp file", () => {
  const dir = tmp();
  const file = path.join(dir, "window-state.json");
  const saved = {
    bounds: { x: 10, y: 20, width: 900, height: 600 },
    displayId: 7,
    isMaximized: true,
    isFullScreen: true,
  };
  saveWindowState(file, saved);
  assert.deepEqual(loadWindowState(file), saved);
  assert.deepEqual(fs.readdirSync(dir), ["window-state.json"]);
});

test("missing, non-JSON and invalid files load as null", () => {
  const dir = tmp();
  assert.equal(loadWindowState(path.join(dir, "none.json")), null);
  const bad = path.join(dir, "bad.json");
  fs.writeFileSync(bad, "{nope");
  assert.equal(loadWindowState(bad), null);
  const nan = path.join(dir, "nan.json");
  fs.writeFileSync(
    nan,
    JSON.stringify({ bounds: { x: 0, y: 0, width: "wide", height: null } }),
  );
  assert.equal(loadWindowState(nan), null);
  const partial = path.join(dir, "partial.json");
  fs.writeFileSync(partial, JSON.stringify({ bounds: { x: 0, y: 0 } }));
  assert.equal(loadWindowState(partial), null);
});
