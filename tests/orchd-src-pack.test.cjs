const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const root = path.join(__dirname, "..");

test("the packaged orchd source carries every include_str! file outside src/", () => {
  const pack = fs.readFileSync(
    path.join(root, "scripts/pack-orchd-src.mjs"),
    "utf8",
  );
  const srcDir = path.join(root, "crates/sushiai-orch/src");
  const files = fs
    .readdirSync(srcDir, { recursive: true })
    .filter((f) => f.endsWith(".rs"));
  const outside = new Set();
  for (const file of files) {
    const text = fs.readFileSync(path.join(srcDir, file), "utf8");
    for (const m of text.matchAll(/include_str!\("([^"]+)"\)/g)) {
      const target = path.relative(
        root,
        path.resolve(path.dirname(path.join(srcDir, file)), m[1]),
      );
      // orch tests fixtures are read only by #[cfg(test)] code, never by a build.
      if (
        !target.startsWith("crates/sushiai-orch/src/") &&
        !target.startsWith("crates/sushiai-orch/tests/")
      )
        outside.add(target.split("/").slice(0, 3).join("/"));
    }
  }
  assert.ok(outside.size > 0);
  for (const dir of outside) assert.ok(pack.includes(`"${dir}"`), dir);
});
