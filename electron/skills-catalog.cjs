const path = require("node:path");
const fs = require("node:fs/promises");
const { runSushiai } = require("./sushiai-run.cjs");

// The rows come from `sushiai skills --json --catalog`: the sushiai binary
// owns the skill folders, the plugin lookup, availability and duplicates. This
// file adds the usage signals and what changed since the previous scan.
const DAY = 24 * 60 * 60 * 1000;
const RECENT_DAYS = 30;
const STALE_DAYS = 180;
const MIN_TIMESTAMP = Date.UTC(2000, 0, 1);

function normalize(value) {
  return String(value || "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function aliases(value) {
  const text = String(value || "");
  return [text, text.split(":").pop()].map(normalize).filter(Boolean);
}

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

async function usageSignals(home) {
  const records = [];
  const record = (provider, name, at, calls, source, extra = {}) => {
    const count = Math.max(0, Math.floor(Number(calls) || 0));
    const timestampValue = timestamp(at);
    if ((!count && !timestampValue) || !name) return;
    records.push({
      provider,
      aliases: aliases(name),
      at: timestampValue,
      calls: count,
      source,
      project: extra.project,
      session: extra.session,
    });
  };

  const eventsPath = path.join(home, ".claude/skill-events.jsonl");
  try {
    const source = await fs.readFile(eventsPath, "utf8");
    for (const line of source.split(/\r?\n/)) {
      try {
        const event = JSON.parse(line);
        record(
          "Claude",
          event.skill,
          Number(event.ts) * 1000,
          1,
          "skill events",
          { project: event.project, session: event.session },
        );
      } catch {
        /* Ignore malformed historical event lines. */
      }
    }
  } catch {
    /* Claude's event log is optional. */
  }

  const cache = await readJson(
    path.join(home, ".claude/coach/skill-usage-cache.json"),
    null,
  );
  for (const [name, value] of Object.entries(cache?.usage || {}))
    record(
      "Claude",
      name,
      Number(value?.lastUsed) * 1000,
      value?.calls,
      "usage cache",
    );
  return records;
}

function usageFor(item, records) {
  const aliasesForItem = new Set(item._aliases);
  const matches = records.filter(
    (record) =>
      record.provider === item.provider &&
      record.aliases.some((key) => aliasesForItem.has(key)),
  );
  const events = matches
    .filter((record) => record.source === "skill events")
    .sort((a, b) => (b.at || 0) - (a.at || 0));
  const cache = matches.filter((record) => record.source === "usage cache");
  const eventCalls = events.reduce((total, record) => total + record.calls, 0);
  const cacheCalls = cache.reduce((total, record) => total + record.calls, 0);
  const latest = matches
    .filter((record) => record.at)
    .sort((a, b) => b.at - a.at)[0];
  return {
    count: Math.max(eventCalls, cacheCalls),
    latest,
    events: events.slice(0, 10).map((record) => ({
      at: record.at,
      project: record.project,
      session: record.session,
    })),
  };
}

/** One catalog row of the binary as the Skills screen's item. `root` stays
 * absolute: the main process checks a change against it. */
function itemOf(home, row) {
  const accessed = timestamp(row.accessedAt);
  return {
    name: row.name,
    description: row.description || "Local agent skill",
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
    lastUsedAt: accessed,
    lastUsedSource: accessed ? "filesystem access" : undefined,
    size: row.size,
    isDuplicate: Boolean(row.isDuplicate),
    duplicateKind: row.duplicateKind,
    duplicateCount: row.duplicateCount,
    duplicateWith: (row.duplicateWith || []).map((file) =>
      displayPath(home, file),
    ),
    _absolutePath: row.file,
    _contentHash: row.contentHash,
    _aliases: [
      ...new Set([...aliases(row.name), ...aliases(path.basename(row.dir))]),
    ],
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
  const previousScanAt =
    Number(previous?.scanAt) ||
    Math.max(
      0,
      ...Object.values(previousItems).map((item) => Number(item?.seenAt) || 0),
    );
  const found = rows.map((row) => itemOf(resolvedHome, row));
  const usage = await usageSignals(resolvedHome);
  for (const item of found) {
    const baseline = previousItems[item._absolutePath];
    const fileAccess = item.lastUsedAt;
    const wasReadByPreviousScan =
      previousScanAt > 0 &&
      fileAccess >= previousScanAt - 1000 &&
      fileAccess <= now + 1000;
    if (wasReadByPreviousScan) {
      item.lastUsedAt = undefined;
      item.lastUsedSource = undefined;
    }
    const itemUsage = usageFor(item, usage);
    item.usageCount = itemUsage.count;
    item.recentUses = itemUsage.events;
    if (itemUsage.latest && itemUsage.latest.at > (item.lastUsedAt || 0)) {
      item.lastUsedAt = itemUsage.latest.at;
      item.lastUsedSource =
        itemUsage.latest.source === "skill events"
          ? "skill event"
          : "usage cache";
    }
    item.usageSource = itemUsage.count
      ? itemUsage.latest?.source || "usage cache"
      : item.lastUsedAt
        ? "filesystem access"
        : undefined;
    const usedAge = item.lastUsedAt ? now - item.lastUsedAt : Infinity;
    const updatedAge = item.updatedAt ? now - item.updatedAt : Infinity;
    item.changed = Boolean(
      baseline && baseline.contentHash !== item._contentHash,
    );
    item.isRecent = usedAge >= 0 && usedAge <= RECENT_DAYS * DAY;
    item.isUnused = !item.lastUsedAt || usedAge > STALE_DAYS * DAY;
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

  return found
    .sort(
      (a, b) =>
        a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) ||
        a.path.localeCompare(b.path),
    )
    .map((item) => {
      delete item._absolutePath;
      delete item._contentHash;
      delete item._aliases;
      return item;
    });
}

module.exports = { scanLocalSkills };
