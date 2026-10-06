// Pushes the desktop's projects and workspaces (as groups) to every daemon's
// catalog replica. The desktop is the writer: each sync is a full, authoritative
// snapshot, sent when a host becomes ready (a new generation) and, debounced,
// after a project or workspace change. Errors are logged, never thrown.
const { appDb, readStore, writeStore } = require("./app-db.cjs");

const DEBOUNCE_MS = 500;

/** The manager host a project folder lives on: "local" or the ssh host id. */
function folderHost(endpoint) {
  return typeof endpoint === "string" && endpoint.startsWith("ssh:")
    ? endpoint.slice(4)
    : "local";
}

/** The rev counter lives in the `store` table: the workspace snapshot owns
 * every `app_state` key and deletes the ones it does not know. */
function revStore(userDataDir) {
  return {
    get: () => Number(readStore(userDataDir, "catalog-sync").rev) || 0,
    set: (rev) => {
      appDb(userDataDir);
      writeStore(userDataDir, "catalog-sync", { rev });
    },
  };
}

/** `projects.list()` gives projects with `folders: [{endpoint, cwd}]`;
 * `workspaces()` the saved workspaces in sidebar order; `store` is
 * `{get(): number, set(rev)}`. */
function createCatalogSync({
  manager,
  projects,
  workspaces,
  store,
  log = (message) => console.warn(message),
  debounceMs = DEBOUNCE_MS,
}) {
  let rev = 0;
  try {
    rev = store.get() || 0;
  } catch (error) {
    log(`catalog sync: cannot read rev: ${error.message}`);
  }
  const nextRev = () => {
    rev += 1;
    try {
      store.set(rev);
    } catch (error) {
      log(`catalog sync: cannot save rev: ${error.message}`);
    }
    return rev;
  };

  async function syncHost(host) {
    try {
      const hello = manager.hello(host);
      if (!hello) return;
      const [all, spaces] = await Promise.all([projects.list(), workspaces()]);
      const revision = nextRev();
      const updatedAt = Date.now();
      const mine = [];
      for (const project of all) {
        const folders = (project.folders || [])
          .filter((folder) => folderHost(folder.endpoint) === host)
          .map((folder) => ({ host: hello.host, path: folder.cwd }));
        if (!folders.length) continue;
        mine.push({
          id: project.id,
          name: project.name || "",
          folders,
          rev: revision,
          updatedAt,
          deleted: false,
        });
      }
      const groups = spaces.map((workspace, order) => ({
        id: String(workspace.id),
        projectId:
          typeof workspace.projectId === "string" ? workspace.projectId : "",
        name: typeof workspace.name === "string" ? workspace.name : "",
        order,
        rev: revision,
        updatedAt,
        deleted: false,
      }));
      await manager.request(host, "projects.sync", {
        host: hello.host,
        projects: mine,
        full: true,
      });
      await manager.request(host, "groups.sync", { groups, full: true });
    } catch (error) {
      log(`catalog sync to ${host} failed: ${error?.message ?? error}`);
    }
  }

  const unsubscribe = manager.on("state", (state) => {
    if (state?.state === "ready") void syncHost(state.host);
  });

  let timer = null;
  function notifyChanged() {
    clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      for (const state of manager.states())
        if (state.state === "ready") void syncHost(state.host);
    }, debounceMs);
    timer.unref?.();
  }

  // Projects already announce their changes; keep any earlier listener.
  const previous = projects.onChange;
  projects.onChange = () => {
    previous?.();
    notifyChanged();
  };

  return {
    notifyChanged,
    syncHost,
    stop() {
      clearTimeout(timer);
      unsubscribe();
      projects.onChange = previous;
    },
  };
}

module.exports = { createCatalogSync, revStore, folderHost };
