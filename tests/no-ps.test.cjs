const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

test("shipped code and test helpers never execute the setuid process-list utility", () => {
  const root = path.join(__dirname, "..");
  const files = (dir) =>
    fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
      const file = path.join(dir, entry.name);
      return entry.isDirectory()
        ? files(file)
        : /\.(?:[cm]?js|tsx?|rs)$/.test(file)
          ? [file]
          : [];
    });
  const offenders = ["src", "electron", "scripts", "tests", "crates"]
    .flatMap((dir) => files(path.join(root, dir)))
    .filter((file) =>
      /["'`](?:\/(?:usr\/)?bin\/)?ps(?=["'`\s])/.test(
        fs.readFileSync(file, "utf8"),
      ),
    );
  assert.deepEqual(
    offenders.map((file) => path.relative(root, file)),
    [],
  );
});
