const path = require("node:path");
const fs = require("node:fs/promises");
const { createHash } = require("node:crypto");

const DAY = 24 * 60 * 60 * 1000;
const RECENT_DAYS = 30;
const STALE_DAYS = 180;
const MAX_PLUGIN_DEPTH = 8;
const MAX_SKILLS = 10000;
const MIN_TIMESTAMP = Date.UTC(2000, 0, 1);

const LOCAL_ROOTS = [
  {
    provider: "Codex",
    harness: "Codex",
    source: "Local",
    relativeRoot: ".codex/skills",
  },
  {
    provider: "Claude",
    harness: "Claude Code",
    source: "Local",
    relativeRoot: ".claude/skills",
  },
  {
    provider: "Agent",
    harness: "Shared agents",
    source: "Local",
    relativeRoot: ".agents/skills",
  },
];

// Keep external harnesses explicit. A recursive search for any directory named
// "skills" also finds vendored catalogs and nested package examples.
const OTHER_ROOTS = [
  { harness: "Gemini", relativeRoot: ".gemini/antigravity/skills" },
  { harness: "Cursor", relativeRoot: ".cursor/skills" },
  { harness: "Cursor", relativeRoot: ".cursor/skills-cursor" },
  { harness: "Windsurf", relativeRoot: ".windsurf/skills" },
  { harness: "OpenCode", relativeRoot: ".opencode/skills" },
  { harness: "Goose", relativeRoot: ".goose/skills" },
  { harness: "Amp", relativeRoot: ".amp/skills" },
  { harness: "Factory", relativeRoot: ".factory/skills" },
  { harness: "Roo", relativeRoot: ".roo/skills" },
  { harness: "Continue", relativeRoot: ".continue/skills" },
  {
    harness: "Terminal Browser",
    relativeRoot: ".local/share/terminal-browser/app/skills",
  },
];

const SCAN_SKIP = new Set([
  ".git",
  ".cache",
  "node_modules",
  "dist",
  "build",
  "file-history",
  "paste-cache",
  "projects",
  "logs",
  "debug",
]);

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

function unquote(value) {
  const trimmed = String(value || "").trim();
  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"')) ||
    (trimmed.startsWith("'") && trimmed.endsWith("'"))
  )
    return trimmed.slice(1, -1);
  return trimmed;
}

function frontMatter(source) {
  const match = String(source || "").match(
    /^\uFEFF?---\s*\r?\n([\s\S]*?)\r?\n---\s*(?:\r?\n|$)/,
  );
  if (!match) return {};
  const lines = match[1].split(/\r?\n/);
  const fields = {};
  for (let index = 0; index < lines.length; index++) {
    const field = lines[index].match(/^([A-Za-z][\w-]*):\s*(.*)$/);
    if (!field) continue;
    const [, key, rawValue] = field;
    if (rawValue === "|" || rawValue === ">") {
      const values = [];
      for (let next = index + 1; next < lines.length; next++) {
        if (/^[A-Za-z][\w-]*:\s*/.test(lines[next])) break;
        if (lines[next].trim()) values.push(lines[next].trim());
        index = next;
      }
      fields[key] = values.join(rawValue === ">" ? " " : "\n");
    } else fields[key] = unquote(rawValue);
  }
  return fields;
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

async function directoryExists(directory) {
  try {
    return (await fs.stat(directory)).isDirectory();
  } catch {
    return false;
  }
}

async function newestDirectory(directory) {
  let entries;
  try {
    entries = await fs.readdir(directory, { withFileTypes: true });
  } catch {
    return undefined;
  }
  const candidates = [];
  for (const entry of entries) {
    if (SCAN_SKIP.has(entry.name)) continue;
    const fullPath = path.join(directory, entry.name);
    try {
      const stat = await fs.stat(fullPath);
      if (stat.isDirectory())
        candidates.push({ path: fullPath, mtimeMs: stat.mtimeMs });
    } catch {
      /* Ignore incomplete cache entries. */
    }
  }
  return candidates.sort(
    (a, b) => b.mtimeMs - a.mtimeMs || a.path.localeCompare(b.path),
  )[0]?.path;
}

function parseCodexConfig(source) {
  const marketplaces = new Map();
  const plugins = new Map();
  const skillConfigs = new Map();
  let section;
  for (const line of String(source || "").split(/\r?\n/)) {
    const skillsConfig = line.match(/^\[\[skills\.config\]\]\s*$/);
    const marketplace = line.match(/^\[marketplaces\.([^\]]+)\]\s*$/);
    const plugin = line.match(/^\[plugins\."([^"]+)"\]\s*$/);
    const table = line.match(/^\[\[?([^\]]+)\]\]?\s*$/);
    if (skillsConfig) section = { kind: "skill" };
    else if (marketplace)
      section = { kind: "marketplace", name: marketplace[1] };
    else if (plugin) section = { kind: "plugin", name: plugin[1] };
    else if (table) section = undefined;
    else if (!section) continue;
    const sourceValue = line.match(/^source\s*=\s*("[^"]*"|'[^']*')\s*$/);
    const skillPath = line.match(/^path\s*=\s*("[^"]*"|'[^']*')\s*$/);
    const enabled = line.match(/^enabled\s*=\s*(true|false)\s*$/);
    if (sourceValue && section.kind === "marketplace")
      marketplaces.set(section.name, unquote(sourceValue[1]));
    if (enabled && section.kind === "plugin")
      plugins.set(section.name, enabled[1] === "true");
    if (skillPath && section.kind === "skill")
      section.path = unquote(skillPath[1]);
    if (enabled && section.kind === "skill" && section.path)
      skillConfigs.set(section.path, enabled[1] === "true");
  }
  return { marketplaces, plugins, skillConfigs };
}

async function codexPluginRoots(home) {
  let config = "";
  try {
    config = await fs.readFile(path.join(home, ".codex/config.toml"), "utf8");
  } catch {
    return [];
  }
  const { marketplaces, plugins } = parseCodexConfig(config);
  const roots = [];
  for (const [pluginKey, enabled] of plugins) {
    const separator = pluginKey.lastIndexOf("@");
    if (separator < 1) continue;
    const pluginName = pluginKey.slice(0, separator);
    const marketplaceName = pluginKey.slice(separator + 1);
    const cacheRoot = path.join(
      home,
      ".codex/plugins/cache",
      marketplaceName,
      pluginName,
    );
    const candidates = [];
    const cachedVersion = await newestDirectory(cacheRoot);
    if (cachedVersion) candidates.push(cachedVersion);
    const marketplaceSource = marketplaces.get(marketplaceName);
    if (marketplaceSource?.startsWith("/")) {
      candidates.push(
        path.join(marketplaceSource, "plugins", pluginName),
        path.join(marketplaceSource, pluginName),
      );
    }
    let root;
    for (const candidate of candidates) {
      if (await directoryExists(candidate)) {
        root = candidate;
        break;
      }
    }
    if (!root) continue;
    roots.push({
      provider: "Codex",
      harness: "Codex",
      source: "Plugin",
      availability: enabled ? "active" : "disabled",
      plugin: pluginKey,
      absoluteRoot: root,
    });
  }
  return roots;
}

async function claudePluginRoots(home) {
  const settings = await readJson(path.join(home, ".claude/settings.json"), {});
  const installed = await readJson(
    path.join(home, ".claude/plugins/installed_plugins.json"),
    {},
  );
  const enabled = settings?.enabledPlugins || {};
  const skillOverrides = Object.fromEntries(
    Object.entries(settings?.skillOverrides || {}).map(([name, value]) => [
      normalize(name),
      value,
    ]),
  );
  const roots = [];
  for (const [pluginKey, installations] of Object.entries(
    installed?.plugins || {},
  )) {
    const installation = [
      ...(Array.isArray(installations) ? installations : []),
    ]
      .filter((entry) => typeof entry?.installPath === "string")
      .sort((a, b) =>
        String(b.lastUpdated || b.installedAt || "").localeCompare(
          String(a.lastUpdated || a.installedAt || ""),
        ),
      )[0];
    if (!installation || !(await directoryExists(installation.installPath)))
      continue;
    roots.push({
      provider: "Claude",
      harness: "Claude Code",
      source: "Plugin",
      availability: enabled[pluginKey] === true ? "active" : "disabled",
      skillOverrides,
      plugin: pluginKey,
      pluginScope: installation.scope || "user",
      absoluteRoot: installation.installPath,
    });
  }
  return roots;
}

async function discoverOtherRoots(home) {
  const roots = await Promise.all(
    OTHER_ROOTS.map(async ({ harness, relativeRoot }) =>
      (await directoryExists(path.join(home, relativeRoot)))
        ? {
            provider: "Other",
            harness,
            source: "External",
            availability: "external",
            relativeRoot,
          }
        : undefined,
    ),
  );
  return roots.filter(Boolean);
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

async function collectSkillFile(filePath, metadata, found, previousItems) {
  if (found.length >= MAX_SKILLS) return;
  let stat;
  try {
    stat = await fs.stat(filePath);
    if (!stat.isFile()) return;
  } catch {
    return;
  }
  const cached = previousItems?.[filePath];
  const canReuse = Boolean(
    cached?.contentHash &&
    Number(cached.mtimeMs) === stat.mtimeMs &&
    Number(cached.size) === stat.size,
  );
  let name;
  let description;
  let hash;
  let cachedAliases;
  if (canReuse) {
    name = cached.name;
    description = cached.description;
    hash = cached.contentHash;
    cachedAliases = cached.aliases;
  } else {
    let source;
    try {
      source = await fs.readFile(filePath, "utf8");
    } catch {
      return;
    }
    const fields = frontMatter(source);
    const directoryName = path.basename(path.dirname(filePath));
    name = fields.name || directoryName;
    description =
      fields.description ||
      source.match(/^#\s+(.+)$/m)?.[1]?.trim() ||
      "Local agent skill";
    hash = createHash("sha256").update(source).digest("hex");
    cachedAliases = [...new Set([...aliases(name), ...aliases(directoryName)])];
  }
  if (!name || !description || !hash) return;
  const override =
    metadata.provider === "Claude" && metadata.source === "Local"
      ? metadata.skillOverrides?.[normalize(name)]
      : undefined;
  const configuredAvailability = metadata.skillConfigs?.get(
    path.resolve(filePath),
  );
  const availability =
    configuredAvailability ||
    (metadata.availability === "active" && override === "off"
      ? "disabled"
      : metadata.availability);
  const disabledBy =
    availability !== "disabled"
      ? undefined
      : metadata.availability === "disabled"
        ? "provider"
        : configuredAvailability === "disabled"
          ? "skill-config"
          : "skill-override";
  found.push({
    name,
    description,
    path: displayPath(metadata.home, filePath),
    provider: metadata.provider,
    harness: metadata.harness,
    source: metadata.source,
    availability,
    disabledBy,
    plugin: metadata.plugin,
    pluginScope: metadata.pluginScope,
    updatedAt: timestamp(stat.mtimeMs),
    lastUsedAt: timestamp(stat.atimeMs),
    lastUsedSource: timestamp(stat.atimeMs) ? "filesystem access" : undefined,
    size: stat.size,
    mtimeMs: stat.mtimeMs,
    contentHash: hash,
    _absolutePath: filePath,
    _aliases: cachedAliases || aliases(name),
  });
}

async function collectRecursiveSkills(root, metadata, found, previousItems) {
  const seen = new Set();
  const seenDirectories = new Set();
  async function walk(directory, depth) {
    if (found.length >= MAX_SKILLS || depth > MAX_PLUGIN_DEPTH) return;
    let realDirectory;
    try {
      realDirectory = await fs.realpath(directory);
    } catch {
      return;
    }
    if (seenDirectories.has(realDirectory)) return;
    seenDirectories.add(realDirectory);
    let entries;
    try {
      entries = await fs.readdir(directory, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      if (found.length >= MAX_SKILLS || SCAN_SKIP.has(entry.name)) continue;
      const fullPath = path.join(directory, entry.name);
      if (entry.name.toLowerCase() === "skill.md") {
        if (!seen.has(fullPath)) {
          seen.add(fullPath);
          await collectSkillFile(fullPath, metadata, found, previousItems);
        }
        continue;
      }
      if (await directoryExists(fullPath)) await walk(fullPath, depth + 1);
    }
  }
  await walk(root, 0);
}

async function collectCanonicalSkills(
  root,
  metadata,
  found,
  previousItems,
  depth = 0,
) {
  if (depth > 2) return;
  let entries;
  try {
    entries = await fs.readdir(root, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    if (found.length >= MAX_SKILLS || SCAN_SKIP.has(entry.name)) continue;
    const fullPath = path.join(root, entry.name);
    if (entry.name.toLowerCase() === "skill.md") {
      await collectSkillFile(fullPath, metadata, found, previousItems);
      continue;
    }
    if (!(await directoryExists(fullPath))) continue;
    if (entry.name === ".system" && metadata.provider === "Codex") {
      await collectCanonicalSkills(
        fullPath,
        metadata,
        found,
        previousItems,
        depth + 1,
      );
      continue;
    }
    await collectSkillFile(
      path.join(fullPath, "SKILL.md"),
      metadata,
      found,
      previousItems,
    );
  }
}

async function collectPluginSkills(root, metadata, found, previousItems) {
  await collectSkillFile(
    path.join(root, "SKILL.md"),
    metadata,
    found,
    previousItems,
  );
  const skillsRoot = path.join(root, "skills");
  if (await directoryExists(skillsRoot))
    await collectRecursiveSkills(skillsRoot, metadata, found, previousItems);
}

function addDuplicateMetadata(items) {
  const byName = new Map();
  const byHash = new Map();
  for (const item of items) {
    const name = normalize(item.name);
    if (name) byName.set(name, [...(byName.get(name) || []), item]);
    byHash.set(item.contentHash, [
      ...(byHash.get(item.contentHash) || []),
      item,
    ]);
  }
  for (const item of items) {
    const sameName = byName.get(normalize(item.name)) || [];
    const sameContent = byHash.get(item.contentHash) || [];
    const peers = [...new Set([...sameName, ...sameContent])].filter(
      (peer) => peer !== item,
    );
    item.isDuplicate = peers.length > 0;
    item.duplicateKind = sameContent.length > 1 ? "exact" : "name";
    item.duplicateCount = peers.length + 1;
    item.duplicateWith = peers.map((peer) => peer.name);
  }
}

async function scanLocalSkills({ home, snapshotFile, now = Date.now() } = {}) {
  const resolvedHome = path.resolve(home || require("node:os").homedir());
  const previous = await readJson(snapshotFile, {});
  const previousItems = previous?.items || previous || {};
  const previousScanAt =
    Number(previous?.scanAt) ||
    Math.max(
      0,
      ...Object.values(previousItems).map((item) => Number(item?.seenAt) || 0),
    );
  const found = [];
  const localRoots = LOCAL_ROOTS.map((root) => ({
    ...root,
    absoluteRoot: path.join(resolvedHome, root.relativeRoot),
    availability: "active",
  }));
  const claudeSettings = await readJson(
    path.join(resolvedHome, ".claude/settings.json"),
    {},
  );
  const claudeSkillOverrides = Object.fromEntries(
    Object.entries(claudeSettings?.skillOverrides || {}).map(
      ([name, value]) => [normalize(name), value],
    ),
  );
  let codexConfig = "";
  try {
    codexConfig = await fs.readFile(
      path.join(resolvedHome, ".codex/config.toml"),
      "utf8",
    );
  } catch {
    /* Codex config is optional. */
  }
  const { skillConfigs: codexSkillConfigs } = parseCodexConfig(codexConfig);
  const codexSkillAvailability = new Map();
  for (const [skillPath, enabled] of codexSkillConfigs) {
    const absolutePath = path.resolve(
      skillPath.startsWith("~/")
        ? path.join(resolvedHome, skillPath.slice(2))
        : path.isAbsolute(skillPath)
          ? skillPath
          : path.join(resolvedHome, skillPath),
    );
    codexSkillAvailability.set(absolutePath, enabled ? "active" : "disabled");
  }
  localRoots.forEach((root) => {
    if (root.provider === "Claude") root.skillOverrides = claudeSkillOverrides;
    if (root.provider === "Codex" || root.provider === "Agent") {
      root.skillConfigs = codexSkillAvailability;
      root.skillConfigRoot = root.absoluteRoot;
    }
  });
  const [claudePlugins, codexPlugins, otherRoots] = await Promise.all([
    claudePluginRoots(resolvedHome),
    codexPluginRoots(resolvedHome),
    discoverOtherRoots(resolvedHome),
  ]);
  for (const root of codexPlugins) root.skillConfigs = codexSkillAvailability;
  const roots = [
    ...localRoots,
    ...claudePlugins,
    ...codexPlugins,
    ...otherRoots,
  ];
  await Promise.all(
    roots.map((root) => {
      const metadata = { home: resolvedHome, ...root };
      const absoluteRoot =
        root.absoluteRoot || path.join(resolvedHome, root.relativeRoot);
      if (root.source === "Plugin")
        return collectPluginSkills(
          absoluteRoot,
          metadata,
          found,
          previousItems,
        );
      if (root.source === "External")
        return collectRecursiveSkills(
          absoluteRoot,
          metadata,
          found,
          previousItems,
        );
      return collectCanonicalSkills(
        absoluteRoot,
        metadata,
        found,
        previousItems,
      );
    }),
  );

  const usage = await usageSignals(resolvedHome);
  for (const item of found) {
    const baseline = previousItems?.[item._absolutePath];
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
      baseline && baseline.contentHash !== item.contentHash,
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
  found.sort(
    (a, b) =>
      a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) ||
      a._absolutePath.localeCompare(b._absolutePath),
  );
  addDuplicateMetadata(found);
  for (const item of found)
    item.needsReview =
      item.changed ||
      item.isDuplicate ||
      item.isUnused ||
      item.isStale ||
      item.availability === "disabled";

  if (snapshotFile) {
    const snapshot = {};
    for (const item of found) {
      snapshot[item._absolutePath] = {
        name: item.name,
        description: item.description,
        contentHash: item.contentHash,
        mtimeMs: item.mtimeMs,
        size: item.size,
        aliases: item._aliases,
        seenAt: now,
      };
    }
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
      delete item.contentHash;
      delete item._absolutePath;
      delete item._aliases;
      delete item.mtimeMs;
      return item;
    });
}

module.exports = { scanLocalSkills };
