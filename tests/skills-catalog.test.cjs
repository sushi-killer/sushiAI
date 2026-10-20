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

test("maps the row's transcript usage and marks recent, stale, unused and changed skills", async () => {
  const home = await fixture();
  const snapshotFile = path.join(home, "app", "skills.json");
  const now = Date.now();
  const old = now - 200 * DAY;
  const at = new Date(now - 2 * 3600 * 1000).toISOString();
  const indexedAt = new Date(now - 60 * 1000).toISOString();
  let hash = "first";
  const { run } = fakeBinary(() => [
    row(home, ".claude/skills/shared", {
      updatedAt: old,
      contentHash: hash,
      description: "x".repeat(40),
      usage: {
        uses90d: 4,
        lastUsedAt: at,
        events: [{ at, project: "/work/app", session: "s1" }],
        indexedAt,
      },
    }),
    row(home, ".codex/skills/unused", {
      harness: "Codex",
      provider: "Codex",
      updatedAt: old,
      usage: { uses90d: 0, lastUsedAt: null, events: [], indexedAt },
    }),
    row(home, ".codex/skills/fresh", {
      harness: "Codex",
      provider: "Codex",
      usage: { uses90d: 0, lastUsedAt: null, events: [], indexedAt: null },
    }),
  ]);
  const options = { binary: "/bin/sushiai", home, snapshotFile, now, run };
  const first = await scanLocalSkills(options);
  const shared = first.find((item) => item.name === "shared");
  assert.equal(shared.lastUsedSource, "transcripts");
  assert.equal(shared.lastUsedAt, Date.parse(at));
  assert.equal(shared.usageCount, 4);
  assert.equal(shared.usageIndexedAt, Date.parse(indexedAt));
  assert.equal(shared.descriptionChars, 40);
  assert.deepEqual(shared.recentUses, [
    {
      provider: "Claude",
      at: Date.parse(at),
      calls: 1,
      source: "transcripts",
      project: "/work/app",
      session: "s1",
    },
  ]);
  assert.equal(shared.isRecent, true);
  assert.equal(shared.isUnused, false);
  assert.equal(shared.changed, false);
  const unused = first.find((item) => item.name === "unused");
  assert.ok(unused.isUnused);
  assert.ok(unused.isStale);
  assert.equal(unused.lastUsedAt, undefined);
  // Before the first index nothing is called unused.
  const fresh = first.find((item) => item.name === "fresh");
  assert.equal(fresh.isUnused, false);
  assert.equal(fresh.usageIndexedAt, undefined);

  hash = "second";
  const second = await scanLocalSkills(options);
  const changed = second.find((item) => item.name === "shared");
  assert.equal(changed.changed, true);
  assert.equal(changed.needsReview, true);
});

test("a row without usage still scans", async () => {
  const home = await fixture();
  const { run } = fakeBinary(() => [row(home, ".claude/skills/bare")]);
  const [bare] = await scanLocalSkills({ binary: "/bin/sushiai", home, run });
  assert.equal(bare.usageCount, 0);
  assert.deepEqual(bare.recentUses, []);
});

test("indexing runs after the catalog, has no timeout, never overlaps, and reports back", async () => {
  const home = await fixture();
  const calls = [];
  let release;
  const gate = new Promise((resolve) => (release = resolve));
  const run = async (binary, args, options) => {
    calls.push({ args, timeout: options.timeout });
    if (args[1] === "usage") {
      await gate;
      return JSON.stringify({ events: 3 });
    }
    return JSON.stringify([row(home, ".claude/skills/one")]);
  };
  let indexed = 0;
  const scan = () =>
    scanLocalSkills({
      binary: "/bin/sushiai",
      home,
      run,
      indexUsage: true,
      onIndexed: () => (indexed += 1),
    });
  await scan();
  await scan();
  const indexRuns = calls.filter((call) => call.args[1] === "usage");
  assert.equal(indexRuns.length, 1);
  assert.deepEqual(indexRuns[0].args, ["skills", "usage", "--index", "--json"]);
  assert.equal(indexRuns[0].timeout, 0);
  assert.equal(indexed, 0);
  release();
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(indexed, 2);
});
