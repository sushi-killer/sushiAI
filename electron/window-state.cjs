const fs = require("node:fs");

const DEFAULT_BOUNDS = { width: 1380, height: 880 };
const MIN_WIDTH = 600;
const MIN_HEIGHT = 440;

function loadWindowState(file) {
  try {
    const raw = JSON.parse(fs.readFileSync(file, "utf8"));
    const b = raw?.bounds;
    if (!b) return null;
    const bounds = { x: b.x, y: b.y, width: b.width, height: b.height };
    for (const value of Object.values(bounds))
      if (typeof value !== "number" || !Number.isFinite(value)) return null;
    return {
      bounds,
      displayId: raw.displayId ?? null,
      isMaximized: raw.isMaximized === true,
      isFullScreen: raw.isFullScreen === true,
    };
  } catch {
    return null;
  }
}

function saveWindowState(file, state) {
  const tmp = `${file}.tmp`;
  fs.writeFileSync(tmp, JSON.stringify(state), { mode: 0o600 });
  fs.renameSync(tmp, file);
}

function overlap(a, b) {
  const w = Math.min(a.x + a.width, b.x + b.width) - Math.max(a.x, b.x);
  const h = Math.min(a.y + a.height, b.y + b.height) - Math.max(a.y, b.y);
  return w > 0 && h > 0 ? w * h : 0;
}

function visibleArea(bounds, areas) {
  // Work areas of separate displays do not overlap, so their sum is the union.
  return areas.reduce((sum, area) => sum + overlap(bounds, area), 0);
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(value, max));
}

function clampBounds(saved, displays, primary) {
  const savedDisplay = displays.find((d) => d.id === saved.displayId);
  const target = savedDisplay ?? primary;
  const area = target.workArea;
  const { x, y } = saved.bounds;
  const width = Math.max(MIN_WIDTH, Math.min(saved.bounds.width, area.width));
  const height = Math.max(
    MIN_HEIGHT,
    Math.min(saved.bounds.height, area.height),
  );
  const resized = { x, y, width, height };
  const visible = visibleArea(
    resized,
    displays.map((d) => d.workArea),
  );
  if (savedDisplay && visible >= (width * height) / 2) return resized;
  return {
    x: clamp(x, area.x, Math.max(area.x, area.x + area.width - width)),
    y: clamp(y, area.y, Math.max(area.y, area.y + area.height - height)),
    width,
    height,
  };
}

module.exports = {
  DEFAULT_BOUNDS,
  loadWindowState,
  saveWindowState,
  clampBounds,
};
