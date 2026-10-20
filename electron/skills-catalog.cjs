const path = require("node:path");
const fs = require("node:fs/promises");
const { runSushiai } = require("./sushiai-run.cjs");

// The rows come from `sushiai skills --json --catalog`: the sushiai binary
// owns the skill folders, the plugin lookup, availability and duplicates. This
// file maps each row's transcript usage and adds what changed since the
// previous scan.
const DAY = 24 * 60 * 60 * 1000;
const RECENT_DAYS = 30;
const STALE_DAYS = 180;
const MIN_TIMESTAMP = Date.UTC(2000, 0, 1);

function displayPath(home, filePath) {
  const prefix = path.resolve(home) + path.sep;
  return filePath.startsWith(prefix)
    ? `~/${filePath.slice(prefix.length)}`
    : filePath;
}

function timestamp(value) {
  return Number.isFinite(value) && value >= MIN_TIMESTAMP ? value : undefined;
}

async function readJson(filePath, fallback) {
  try {
    return JSON.parse(await fs.readFile(filePath, "utf8"));
  } catch {
    return fallback;
  }
}

function parseTime(value) {
  return timestamp(Date.parse(value || ""));
}

/** A row's `usage` as the outward fields the screens read. */
function usageOf(row) {
  const usage = row.usage || {};
  const events = Array.isArray(usage.events) ? usage.events : [];
  const lastUsedAt = parseTime(usage.lastUsedAt);
  return {
    usageCount: Math.max(0, Math.floor(Number(usage.uses90d) || 0)),
    recentUses: events
      .map((event) => ({
        provider: row.provider,
        at: parseTime(event?.at),
        calls: 1,
        source: "transcripts",
        project: event?.project || undefined,
        session: event?.session || undefined,
      }))
      .filter((use) => use.at),
    lastUsedAt,
    lastUsedSource: lastUsedAt ? "transcripts" : undefined,
    usageIndexedAt: parseTime(usage.indexedAt),
  };
}

/** One catalog row of the binary as the Skills screen's item. `root` stays
 * absolute: the main process checks a change against it. */
function itemOf(home, row) {
  return {
    name: row.name,
    description: row.description || "Local agent skill",
    descriptionChars: (row.description || "").length,
    path: displayPath(home, row.file),
    provider: row.provider,
    harness: row.harness,
    source: row.source,
    scope: row.scope,
    availability: row.availability,
    disabledBy: row.disabledBy,
    plugin: row.plugin,
    pluginScope: row.pluginScope,
    root: row.root,
    updatedAt: timestamp(row.updatedAt),
    ...usageOf(row),
    size: row.size,
    isDuplicate: Boolean(row.isDuplicate),
    duplicateKind: row.duplicateKind,
    duplicateCount: row.duplicateCount,
    duplicateWith: (row.duplicateWith || []).map((file) =>
      displayPath(home, file),
    ),
    _absolutePath: row.file,
    _contentHash: row.contentHash,
  };
}

/** The Skills screen's catalog: the binary's rows for `home`, with usage and
 * change signals. `run(binary, args, { env })` is replaceable in tests. */
async function scanLocalSkills({
  binary,
  home,
  env = process.env,
  snapshotFile,
  now = Date.now(),
  run = runSushiai,
  indexUsage = false,
  onIndexed = () => {},
} = {}) {
  if (!binary) throw new Error("The sushiai binary was not found.");
  const resolvedHome = path.resolve(
    home || env.HOME || require("node:os").homedir(),
  );
  const rows = JSON.parse(
    await run(binary, ["skills", "--json", "--catalog"], {
      env: { ...env, HOME: resolvedHome },
    }),
  );
  const previous = await readJson(snapshotFile, {});
  const previousItems = previous?.items || {};
  const found = rows.map((row) => itemOf(resolvedHome, row));
  for (const item of found) {
    const baseline = previousItems[item._absolutePath];
    const updatedAge = item.updatedAt ? now - item.updatedAt : Infinity;
    const usedAge = item.lastUsedAt ? now - item.lastUsedAt : Infinity;
    item.changed = Boolean(
      baseline && baseline.contentHash !== item._contentHash,
    );
    item.isRecent = usedAge >= 0 && usedAge <= RECENT_DAYS * DAY;
    item.isUnused = Boolean(item.usageIndexedAt) && item.usageCount === 0;
    item.isStale = !item.updatedAt || updatedAge > STALE_DAYS * DAY;
    item.needsReview =
      item.changed ||
      item.isDuplicate ||
      item.isUnused ||
      item.isStale ||
      item.availability === "disabled";
  }

  if (snapshotFile) {
    const snapshot = {};
    for (const item of found)
      snapshot[item._absolutePath] = {
        name: item.name,
        contentHash: item._contentHash,
        seenAt: now,
      };
    try {
      await fs.mkdir(path.dirname(snapshotFile), { recursive: true });
      await fs.writeFile(
        snapshotFile,
        JSON.stringify({ scanAt: now, items: snapshot }),
        "utf8",
      );
    } catch {
      /* Catalog remains useful if the app data directory is unavailable. */
    }
  }

  // The catalog is served first; the index runs after it and tells the caller
  // when its numbers are ready to be read again.
  if (indexUsage)
    indexSkillUsage({
      binary,
      env: { ...env, HOME: resolvedHome },
      run,
    }).then(onIndexed, (error) => {
      // One line, never the transcripts' text.
      console.error(
        `Skill usage index failed: ${String(error?.message || error)
          .split("\n")[0]
          .slice(0, 200)}`,
      );
    });

  return found
    .sort(
      (a, b) =>
        a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) ||
        a.path.localeCompare(b.path),
    )
    .map((item) => {
      delete item._absolutePath;
      delete item._contentHash;
      return item;
    });
}

let indexRun;

/** Reads the agents' transcripts into the binary's usage index. The first run
 * takes minutes, so it has no timeout; two runs never overlap (a second call
 * shares the first). Resolves to the binary's summary. */
function indexSkillUsage({ binary, env = process.env, run = runSushiai } = {}) {
  indexRun ||= run(binary, ["skills", "usage", "--index", "--json"], {
    env,
    timeout: 0,
  })
    .then((text) => JSON.parse(text))
    .finally(() => {
      indexRun = undefined;
    });
  return indexRun;
}

module.exports = { scanLocalSkills, indexSkillUsage };
