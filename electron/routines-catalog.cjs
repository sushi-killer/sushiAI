"use strict";

// What the Routines page and the daemon need to know about hosts, harnesses
// and projects. Pure: the caller supplies what it read (hosts, login probes,
// model lists, projects, orchestrator settings), so nothing here touches disk
// or a connection.

const FALLBACK_AGENT = { harness: "claude", model: "sonnet", effort: "medium" };
const EFFORTS = ["low", "medium", "high"];
// Claude's own scale is wider; Codex offers the first three.
const CLAUDE_ONLY_EFFORTS = ["xhigh", "max"];

/** The levels the picker offers for a harness. */
function effortsFor(harness) {
  return harness === "claude" ? [...EFFORTS, ...CLAUDE_ONLY_EFFORTS] : EFFORTS;
}

function modelRows(list) {
  return (Array.isArray(list) ? list : []).map((model) => ({
    id: String(model.id),
    label: String(model.label || model.id),
  }));
}

/** One harness of one host. `fact` is what the login probe saw (`null` when
 * the host could not be probed: login is then unknown, not denied). */
function harnessEntry(fact, models) {
  return {
    installed: fact ? fact.installed !== false : null,
    loggedIn: fact ? fact.loggedIn === true : null,
    models: modelRows(models),
  };
}

/** The default agent of a new routine: the Standard tier's route of the
 * orchestrator settings, or Claude Sonnet at medium when there is none. */
function standardAgent(settings) {
  const id = settings?.tiers?.standard;
  const route = (settings?.routes || []).find((item) => item.id === id);
  if (!route || !["claude", "codex"].includes(route.harness))
    return { ...FALLBACK_AGENT };
  const effort = effortsFor(route.harness).includes(route.effort)
    ? route.effort
    : FALLBACK_AGENT.effort;
  return {
    harness: route.harness,
    model: route.model || "",
    effort,
  };
}

/**
 * The catalog of the desktop: every host with its harnesses and models, every
 * project with its folders per host, the default agent.
 *
 * hosts:    [{ id: "local" | "ssh:<profile>", name, state, routines, hostName }]
 * facts:    { [hostId]: { claude?: {installed, loggedIn}, codex?: {...}, codexModels?: [] } }
 * models:   `chatModels()`: this Mac's lists (`claude.models`, `codex.models`)
 * projects: `projects.list()` with `folders: [{ endpoint, cwd }]`
 */
function assembleCatalog({ hosts, facts = {}, models, projects, settings }) {
  const claudeModels = models?.claude?.models || [];
  return {
    hosts: hosts.map((host) => {
      const fact = facts[host.id] || null;
      const codexModels =
        host.id === "local"
          ? models?.codex?.models || []
          : fact?.codexModels || [];
      return {
        id: host.id,
        name: host.name,
        state: host.state,
        routines: host.routines === true,
        hostName: host.hostName || "",
        harnesses: {
          claude: harnessEntry(fact?.claude ?? null, claudeModels),
          codex: harnessEntry(fact?.codex ?? null, codexModels),
        },
      };
    }),
    projects: projects.map((project) => ({
      id: String(project.id),
      name: String(project.name || ""),
      purpose: String(project.purpose || ""),
      remote: String(project.git?.url || ""),
      folders: (project.folders || []).map((folder) => ({
        host: folder.endpoint === "local" ? "local" : String(folder.endpoint),
        path: String(folder.cwd),
      })),
    })),
    defaults: { agent: standardAgent(settings) },
  };
}

/**
 * The catalog the daemon of `hostId` validates against: only that host, named
 * the way its own folders are (`hostName`, the name in its hello), and only the
 * folders on it. `models` is the list of ids a model must come from; a harness
 * with no known list is absent, so the daemon treats it as unknown.
 */
function daemonCatalog(catalog, hostId) {
  const host = catalog.hosts.find((item) => item.id === hostId);
  if (!host) return { hosts: [], projects: [] };
  const name = host.hostName || "local";
  const harnesses = {};
  for (const [harness, entry] of Object.entries(host.harnesses)) {
    harnesses[harness] = {
      ...(entry.loggedIn === null ? {} : { loggedIn: entry.loggedIn }),
      ...(entry.models.length ? { models: entry.models.map((m) => m.id) } : {}),
    };
  }
  return {
    hosts: [{ id: name, harnesses }],
    projects: catalog.projects.map((project) => ({
      id: project.id,
      name: project.name,
      ...(project.purpose ? { purpose: project.purpose } : {}),
      ...(project.remote ? { remote: project.remote } : {}),
      folders: project.folders
        .filter((folder) => folder.host === hostId)
        .map((folder) => ({ host: name, path: folder.path })),
    })),
  };
}

module.exports = {
  assembleCatalog,
  daemonCatalog,
  standardAgent,
  effortsFor,
  FALLBACK_AGENT,
};
