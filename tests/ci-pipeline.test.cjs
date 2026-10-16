const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { pathToFileURL } = require("node:url");

const root = path.join(__dirname, "..");
const pkg = JSON.parse(
  fs.readFileSync(path.join(root, "package.json"), "utf8"),
);

function expand(command, seen = new Set()) {
  return command
    .split("&&")
    .map((part) => {
      const p = part.trim();
      const m = /^npm (?:run )?([\w:-]+)$/.exec(p);
      if (m && pkg.scripts[m[1]] && !seen.has(m[1])) {
        return expand(pkg.scripts[m[1]], new Set(seen).add(m[1]));
      }
      return p;
    })
    .join(" && ");
}

test("a pull request with docs alone skips the builds; Rust jobs need Rust", async () => {
  const { classify } = await import(
    pathToFileURL(path.join(root, "scripts/ci-changes.mjs")).href
  );
  const docs = { code: false, rust: false };
  assert.deepEqual(classify(["README.md", "docs/INSTALL.md"]), docs);
  assert.deepEqual(classify(["docs/releases/unreleased/x.md"]), docs);
  // Markdown that tests or tools read is code.
  for (const file of [
    ".agents/skills/ship-pr/SKILL.md",
    "crates/sushiai-orch/README.md",
    "tests/fixtures/a.md",
  ]) {
    assert.deepEqual(classify([file]).code, true, file);
  }
  assert.deepEqual(classify(["README.md", "src/App.tsx"]), {
    code: true,
    rust: false,
  });
  for (const file of [
    "crates/sushiai-orch/src/lib.rs",
    "Cargo.lock",
    ".github/workflows/ci.yml",
    "scripts/nextest-sushiai-bin.mjs",
    "electron/test-fixtures/mcp-home/x.json",
  ]) {
    assert.deepEqual(classify([file]), { code: true, rust: true }, file);
  }
  // No diff to read (a push, a new branch): everything runs.
  assert.deepEqual(classify([]), { code: true, rust: true });
});

test("npm run ci still runs every check", async () => {
  assert.equal(pkg.scripts.ci, "node scripts/ci.mjs");
  assert.equal(pkg.scripts["ci:web"], undefined);
  const { lanes } = await import(
    pathToFileURL(path.join(root, "scripts/ci.mjs")).href
  );
  assert.ok(lanes.length >= 2, "at least two lanes");
  const all = lanes
    .flatMap((l) => l.commands)
    .map((c) => expand(c))
    .join(" && ");
  const needles = [
    "vite build",
    "cargo build -p sushiai",
    "node --test",
    "check-conventions.mjs",
    "eslint .",
    "prettier --check",
    "git diff --check",
    "cargo fmt --check",
    "cargo clippy --workspace --all-targets -- -D warnings",
    "test-rust.mjs",
    "check-crate-deps.mjs",
  ];
  for (const needle of needles) assert.ok(all.includes(needle), needle);
});

test("npm run ci runs only the lanes a change needs", async () => {
  const { lanes, lanesFor } = await import(
    pathToFileURL(path.join(root, "scripts/ci.mjs")).href
  );
  const names = (list) => list.map((l) => l.name);
  const commands = (list) => list.flatMap((l) => l.commands).join(" && ");
  assert.deepEqual(names(lanesFor({ code: true, rust: true })), names(lanes));
  assert.deepEqual(names(lanesFor({ code: false, rust: false })), [
    "static",
    "conventions",
  ]);
  const web = lanesFor({ code: true, rust: false });
  assert.ok(!names(web).includes("rust lint"));
  assert.doesNotMatch(commands(web), /test-rust/);
  assert.match(commands(web), /npm test/);
});

test("runLanes runs lanes side by side and stops a lane at its first failure", async () => {
  const { runLanes } = await import(
    pathToFileURL(path.join(root, "scripts/lib/run-lanes.mjs")).href
  );
  const sleep = "node -e 'setTimeout(()=>{},1500)'";
  const start = Date.now();
  const code = await runLanes(
    [
      { name: "a", commands: [sleep] },
      { name: "b", commands: [sleep] },
    ],
    { quiet: true },
  );
  assert.equal(code, 0);
  assert.ok(Date.now() - start < 2900, "lanes overlapped");

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "lanes-"));
  const marker = path.join(dir, "ran");
  const failing = await runLanes(
    [
      {
        name: "x",
        commands: [
          "node -e 'process.exit(3)'",
          `node -e "require('fs').writeFileSync(process.argv[1],'1')" ${marker}`,
        ],
      },
      { name: "y", commands: ["node -e '0'"] },
    ],
    { quiet: true },
  );
  assert.notEqual(failing, 0);
  assert.equal(fs.existsSync(marker), false);
  fs.rmSync(dir, { recursive: true, force: true });
});
