const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

// orchd must build and pass its tests on Linux too. macOS-only facilities may
// appear only behind cfg(target_os = "macos") (or in a comment).
const MACOS_ONLY =
  /sandbox-exec|\/private\/(var|tmp)|Library\/Caches|\/opt\/homebrew/;
const GUARD = /target_os\s*=\s*"macos"/;
const WINDOW = 25;

function rustFiles(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory())
      return entry.name === "target" ? [] : rustFiles(full);
    return entry.name.endsWith(".rs") ? [full] : [];
  });
}

test('orchd has no macOS-only code outside cfg(target_os = "macos")', () => {
  const root = path.join(__dirname, "..", "crates", "sushiai-orch");
  const offenders = [];
  for (const file of rustFiles(root)) {
    const lines = fs.readFileSync(file, "utf8").split("\n");
    lines.forEach((line, index) => {
      if (!MACOS_ONLY.test(line) || /^\s*(\/\/|\*|\/\*)/.test(line)) return;
      const context = lines.slice(Math.max(0, index - WINDOW), index + 1);
      if (!context.some((text) => GUARD.test(text)))
        offenders.push(
          `${path.relative(root, file)}:${index + 1}: ${line.trim()}`,
        );
    });
  }
  assert.deepEqual(offenders, []);
});
