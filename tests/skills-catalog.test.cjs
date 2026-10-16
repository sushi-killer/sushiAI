// The Skills screen's catalog: the sushiai binary's rows (replaced by a fake
// here; tests/skills-catalog-binary.test.cjs runs the real one) with the usage
// and change signals this file adds.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { scanLocalSkills } = require("../electron/skills-catalog.cjs");

const DAY = 24 * 60 * 60 * 1000;

async function fixture() {
  return fs.mkdtemp(path.join(os.tmpdir(), "sushiai-skills-"));
}

function row(home, relativeDir, fields = {}) {
  const dir = path.join(home, relativeDir);
  return {
    name: path.basename(dir),
    description: "A skill",
    harness: "Claude Code",
    provider: "Claude",
    source: "Local",
    scope: "user",
    availability: "active",
    file: path.join(dir, "SKILL.md"),
    dir,
    root: path.dirname(dir),
    scripts: [],
    size: 10,
    updatedAt: Date.now(),
    contentHash: "a",
    isDuplicate: false,
    duplicateCount: 1,
    duplicateWith: [],
    ...fields,
  };
}

/** A fake binary that answers `skills --json --catalog` with `rows()`. */
function fakeBinary(rows) {
  const calls = [];
  const run = async (binary, args, { env }) => {
    calls.push({ binary, args, home: env.HOME });
    return JSON.stringify(rows());
  };
  return { run, calls };
}

test("asks the binary for the catalog of the home and shows paths under ~", async () => {
  const home = await fixture();
  const rows = [
    row(home, ".claude/skills/one", {
      description: "",
      isDuplicate: true,
      duplicateKind: "name",
      duplicateCount: 2,
      duplicateWith: [path.join(home, ".claude/skills/two/SKILL.md")],
    }),
    row(home, ".codex/skills/plugin-one", {
      harness: "Codex",
      provider: "Codex",
      source: "Plugin",
      plugin: "tool@market",
      availability: "disabled",
      disabledBy: "provider",
    }),
  ];
  const { run, calls } = fakeBinary(() => rows);
  const items = await scanLocalSkills({
    binary: "/bin/sushiai",
    home,
    env: { PATH: "/usr/bin" },
    run,
  });
  assert.deepEqual(calls, [
    {
      binary: "/bin/sushiai",
      args: ["skills", "--json", "--catalog"],
      home,
    },
  ]);
  const [one, plugin] = items;
  assert.equal(one.path, "~/.claude/skills/one/SKILL.md");
  assert.equal(one.root, path.join(home, ".claude/skills"));
  assert.equal(one.description, "Local agent skill");
  assert.deepEqual(one.duplicateWith, ["~/.claude/skills/two/SKILL.md"]);
  assert.ok(one.needsReview);
  assert.equal(plugin.availability, "disabled");
  assert.equal(plugin.plugin, "tool@market");
  assert.ok(items.every((item) => !("_absolutePath" in item)));
});

test("a missing binary is an error, not an empty catalog", async () => {
  await assert.rejects(scanLocalSkills({ binary: null }), /binary/);
});

test("uses Claude usage events and marks recent, stale, unused and changed skills", async () => {
  const home = await fixture();
  const snapshotFile = path.join(home, "app", "skills.json");
  const now = Date.now();
  const old = now - 200 * DAY;
  let hash = "first";
  const { run } = fakeBinary(() => [
    row(home, ".claude/skills/shared", {
      updatedAt: old,
      contentHash: hash,
    }),
    row(home, ".codex/skills/unused", {
      harness: "Codex",
      provider: "Codex",
      updatedAt: old,
    }),
  ]);
  await fs.mkdir(path.join(home, ".claude/coach"), { recursive: true });
  await fs.writeFile(
    path.join(home, ".claude/skill-events.jsonl"),
    JSON.stringify({
      ts: Math.floor((now - 2 * 3600 * 1000) / 1000),
      skill: "shared",
    }) + "\n",
  );
  await fs.writeFile(
    path.join(home, ".claude/coach/skill-usage-cache.json"),
    JSON.stringify({
      usage: { shared: { calls: 4, lastUsed: (now - 3 * 3600 * 1000) / 1000 } },
    }),
  );
  const options = { binary: "/bin/sushiai", home, snapshotFile, now, run };
  const first = await scanLocalSkills(options);
  const shared = first.find((item) => item.name === "shared");
  assert.equal(shared.lastUsedSource, "skill event");
  assert.equal(shared.usageCount, 4);
  assert.equal(shared.recentUses.length, 1);
  assert.equal(shared.isRecent, true);
  assert.equal(shared.changed, false);
  const unused = first.find((item) => item.name === "unused");
  assert.ok(unused.isUnused);
  assert.ok(unused.isStale);

  hash = "second";
  const second = await scanLocalSkills(options);
  const changed = second.find((item) => item.name === "shared");
  assert.equal(changed.changed, true);
  assert.equal(changed.needsReview, true);
});
