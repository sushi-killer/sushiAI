// docs/MAP.md is the first lookup for an agent in this repository. These
// tests keep it complete (every source area is named) and keep its CI path
// check honest (a path that no longer exists fails, with the path named).
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const root = path.join(__dirname, "..");
const read = (file) => fs.readFileSync(path.join(root, file), "utf8");

function check(env) {
  return spawnSync(process.execPath, ["scripts/check-conventions.mjs"], {
    cwd: root,
    encoding: "utf8",
    env: { ...process.env, ...env },
  });
}

// Inline code spans outside fenced blocks, one trailing slash dropped so
// `src/app/` and `src/app` name the same area.
function mapSpans() {
  const inline = read("docs/MAP.md").replace(/```[\s\S]*?```/g, "");
  return new Set(
    [...inline.matchAll(/`([^`\n]+)`/g)].map(([, span]) =>
      span.trim().replace(/\/$/, ""),
    ),
  );
}

const subdirs = (dir) =>
  fs
    .readdirSync(path.join(root, dir), { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && !entry.name.startsWith("."))
    .map((entry) => `${dir}/${entry.name}`);

test("docs/MAP.md names every source directory and every skill", () => {
  const spans = mapSpans();
  const missing = ["src", "electron", "crates", ".agents/skills"]
    .flatMap((dir) => subdirs(dir))
    .filter((dir) => !spans.has(dir));
  assert.deepEqual(missing, []);
});

test("docs/MAP.md names the key entry files and rule owners", () => {
  const spans = mapSpans();
  const required = [
    "src/main.tsx",
    "src/App.tsx",
    "src/bridge.ts",
    "src/app/SectionPage.tsx",
    "src/extensions/modules.ts",
    "electron/main.cjs",
    "electron/preload.cjs",
    "electron/ipc",
    "electron/extensions/manifest.cjs",
    "crates/ARCHITECTURE.md",
    "crates/sushiai/src/main.rs",
    "crates/sushiai-orch/src/engine",
    "crates/sushiai-orch/src/engine/mod.rs",
    "crates/sushiai-orch/src/engine/attempts/brief.rs",
    "crates/sushiai-orch/src/policy/settings.rs",
    "crates/sushiai-orch/prompts/orchestrator.yaml",
    "tests",
    "scripts/check-conventions.mjs",
    "scripts/check-crate-deps.mjs",
    "scripts/release.mjs",
    "scripts/test-rust.mjs",
    "scripts/hooks",
    ".claude/settings.json",
    ".github/workflows/ci.yml",
    ".agents/skills",
    "AGENTS.md",
    "docs/architecture.md",
    "docs/orchestrator-settings.md",
  ];
  assert.deepEqual(
    required.filter((span) => !spans.has(span)),
    [],
  );
});

test("docs/MAP.md answers the common where-is questions", () => {
  const section =
    read("docs/MAP.md")
      .split(/^## Where is/m)[1]
      ?.split(/^## /m)[0] ?? "";
  assert.ok(section, "docs/MAP.md has no '## Where is ...' section");
  for (const answer of [
    "crates/sushiai-orch/src/policy/settings.rs",
    "electron/preload.cjs",
    "electron/ipc",
    "crates/sushiai-orch/src/engine/attempts/brief.rs",
    "scripts/check-conventions.mjs",
    "node --test tests/",
    "cargo test -p ",
  ])
    assert.ok(section.includes(answer), `Where is section lacks ${answer}`);
});

test("AGENTS.md links the map under Read when relevant", () => {
  const section = read("AGENTS.md").split(/^## Read when relevant$/m)[1] ?? "";
  assert.match(section, /`docs\/MAP\.md`/);
});

test("a path in docs/MAP.md that does not exist fails the check", () => {
  const tree = fs.mkdtempSync(path.join(os.tmpdir(), "docs-map-"));
  const write = (file, text) => {
    fs.mkdirSync(path.dirname(path.join(tree, file)), { recursive: true });
    fs.writeFileSync(path.join(tree, file), text);
  };
  try {
    const missingMap = check({ MAP_ROOT_OVERRIDE: tree });
    assert.equal(missingMap.status, 1);
    assert.match(missingMap.stderr, /docs\/MAP\.md is missing/);

    write("src/real.ts", "export const real = 1;\n");
    write("crates/x/Cargo.toml", "[package]\n");
    write("README.md", "# x\n");
    write(
      "docs/MAP.md",
      [
        "# Map",
        "",
        "- `src/real.ts` and `src/real.ts:12` are one file.",
        "- `src/` and `crates/x` are directories, `README.md` is a file.",
        "- Not paths: `npm run ci`, `orch.*`, `tests/*.test.cjs`.",
        "- Not paths: `settings.get`, `<slug>/notes.md`, `~/.sushiai/bin`.",
        "- Not paths: `./node_modules/.bin/prettier`, `https://example.com/a`.",
        "",
        "```sh",
        "cat `src/fenced-only.ts`",
        "```",
        "",
      ].join("\n"),
    );
    const clean = check({ MAP_ROOT_OVERRIDE: tree });
    assert.equal(clean.status, 0, clean.stderr);

    fs.rmSync(path.join(tree, "src/real.ts"));
    const deleted = check({ MAP_ROOT_OVERRIDE: tree });
    assert.equal(deleted.status, 1);
    assert.match(
      deleted.stderr,
      /docs\/MAP\.md:3 names src\/real\.ts, which does not exist/,
    );
    write("src/real.ts", "export const real = 1;\n");

    fs.appendFileSync(
      path.join(tree, "docs/MAP.md"),
      "- `gone.md` and `crates/x/gone.rs` are gone.\n",
    );
    const appended = check({ MAP_ROOT_OVERRIDE: tree });
    assert.equal(appended.status, 1);
    assert.match(appended.stderr, /names gone\.md, which does not exist/);
    assert.match(
      appended.stderr,
      /names crates\/x\/gone\.rs, which does not exist/,
    );
  } finally {
    fs.rmSync(tree, { recursive: true, force: true });
  }
});
