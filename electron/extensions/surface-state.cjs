const path = require("node:path");
const {
  DAMAGED,
  appDb,
  putStore,
  readStore,
  transaction,
} = require("../app-db.cjs");

const ID = /^[a-z0-9][a-z0-9._-]*$/;
const MAX_SCOPE = 512;
const MAX_BYTES = 512 * 1024;

/** Durable state for declarative surfaces: one `surface-state` row per extension in sushiai.db, nested
 * by surface, state version and scope.
 *
 * The nesting is structural rather than a joined key because surface ids may
 * contain dots and a scope is a path - a flat key could not be parsed back
 * apart, and aggregating one surface across projects needs exactly that. */
class SurfaceStateStore {
  constructor(userDataDir) {
    if (typeof userDataDir !== "string" || !path.isAbsolute(userDataDir))
      throw new Error("Surface state directory must be absolute.");
    this.userDataDir = userDataDir;
    this.queues = new Map();
  }

  /** Drops the state of built-ins the app no longer ships (`keep` is the set
   * of extension ids that exist now). */
  async forgetRetired(keep) {
    const all = readStore(this.userDataDir, "surface-state", { damaged: true });
    if (Object.values(all).includes(DAMAGED)) return;
    const next = Object.fromEntries(
      Object.entries(all).filter(
        ([id]) => !id.startsWith("builtin.") || keep.has(id),
      ),
    );
    if (Object.keys(next).length === Object.keys(all).length) return;
    const db = appDb(this.userDataDir);
    transaction(db, () => putStore(db, "surface-state", next));
  }

  /** The queue key of an extension's state. */
  file(extensionId) {
    if (typeof extensionId !== "string" || !ID.test(extensionId))
      throw new Error("Invalid extension id.");
    return extensionId;
  }

  static place(surfaceId, version, scope) {
    if (typeof surfaceId !== "string" || !ID.test(surfaceId))
      throw new Error("Invalid surface id.");
    if (!Number.isInteger(version) || version < 1 || version > 1e6)
      throw new Error("Invalid surface state version.");
    if (scope !== undefined) {
      if (typeof scope !== "string" || !scope || scope.length > MAX_SCOPE)
        throw new Error("Invalid surface state scope.");
    }
    return { surfaceId, version: String(version), scope };
  }

  /** Anything already queued for this file, so a read never overtakes a
   * write that has been issued but not yet landed. */
  async settled(extensionId) {
    await (this.queues.get(this.file(extensionId)) || Promise.resolve()).catch(
      () => {},
    );
  }

  async all(extensionId) {
    const value = readStore(this.userDataDir, "surface-state", {
      damaged: true,
    })[this.file(extensionId)];
    if (value === undefined) return {};
    if (value === DAMAGED || !plain(value))
      throw new DamagedSurfaceState(extensionId);
    return value;
  }

  async read(extensionId, surfaceId, version, scope) {
    const at = SurfaceStateStore.place(surfaceId, version, scope);
    await this.settled(extensionId);
    const slice = branch(await this.all(extensionId), at.surfaceId, at.version);
    return Object.prototype.hasOwnProperty.call(slice, at.scope)
      ? slice[at.scope]
      : null;
  }

  /** Every scope of one surface at one version, for a surface that declared
   * itself an aggregate. Never crosses surfaces or extensions. */
  async aggregate(extensionId, surfaceId, version) {
    const at = SurfaceStateStore.place(surfaceId, version);
    await this.settled(extensionId);
    const slice = branch(await this.all(extensionId), at.surfaceId, at.version);
    return Object.entries(slice).map(([scope, records]) => ({
      scope,
      records,
    }));
  }

  /** State left behind by older state versions, so it can be shown and cleared
   * rather than accumulating invisibly. */
  async versions(extensionId, surfaceId) {
    const at = SurfaceStateStore.place(surfaceId, 1);
    const surface = (await this.all(extensionId))[at.surfaceId];
    if (!plain(surface)) return [];
    return Object.entries(surface)
      .filter(([, value]) => plain(value))
      .map(([version, value]) => ({
        version: Number(version),
        scopes: Object.keys(value).length,
        bytes: JSON.stringify(value).length,
      }));
  }

  async write(extensionId, surfaceId, version, scope, value) {
    const at = SurfaceStateStore.place(surfaceId, version, scope);
    const encoded = JSON.stringify(value ?? null);
    if (encoded.length > MAX_BYTES)
      throw new Error(`Surface state is larger than ${MAX_BYTES / 1024} KiB.`);
    const file = this.file(extensionId);
    // Chained so a burst of edits cannot lose an earlier read-modify-write.
    const run = (this.queues.get(file) || Promise.resolve())
      .catch(() => {})
      .then(async () => {
        const state = await this.all(extensionId);
        if (!plain(state[at.surfaceId])) state[at.surfaceId] = {};
        if (!plain(state[at.surfaceId][at.version]))
          state[at.surfaceId][at.version] = {};
        const slice = state[at.surfaceId][at.version];
        if (value === null || value === undefined) delete slice[at.scope];
        else slice[at.scope] = JSON.parse(encoded);
        if (!Object.keys(slice).length) delete state[at.surfaceId][at.version];
        if (!Object.keys(state[at.surfaceId]).length)
          delete state[at.surfaceId];
        const db = appDb(this.userDataDir);
        transaction(db, () =>
          putStore(db, "surface-state", {
            ...readStore(this.userDataDir, "surface-state"),
            [file]: state,
          }),
        );
      });
    this.queues.set(file, run);
    await run;
  }
}

/** Thrown rather than silently starting empty: the caller shows the surface
 * read-only so a hand-edit with a stray comma cannot be overwritten. */
class DamagedSurfaceState extends Error {
  constructor(extensionId) {
    super(
      `Saved state for ${extensionId} could not be read. It is shown read-only.`,
    );
    this.code = "SURFACE_STATE_DAMAGED";
  }
}

const plain = (value) =>
  Boolean(value) && typeof value === "object" && !Array.isArray(value);

function branch(state, surfaceId, version) {
  const surface = state[surfaceId];
  if (!plain(surface)) return {};
  const slice = surface[version];
  return plain(slice) ? slice : {};
}

module.exports = { SurfaceStateStore, DamagedSurfaceState, MAX_BYTES };
