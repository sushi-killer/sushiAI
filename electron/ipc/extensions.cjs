const MAX_RECORDS = 1000;

/** The surface a request names, if it exists and is switched on. State is
 * addressed by (extension, surface, version, scope), so main can check the
 * address against the manifest rather than trusting whatever the renderer
 * sends - this is the seam a richer record schema will validate through. */
function locate(extensions, extensionId, surfaceId) {
  const surface = extensions?.activeSurface(extensionId, surfaceId);
  if (!surface)
    throw new Error(
      `No active extension surface at ${extensionId}/${surfaceId}.`,
    );
  return surface;
}

function checkAddress(surface, version, scope) {
  if (version !== surface.stateVersion)
    throw new Error(
      `Surface ${surface.id} keeps version ${surface.stateVersion} state, not ${version}.`,
    );
  if (surface.stateScope === "global" && scope !== "global")
    throw new Error(`Surface ${surface.id} keeps one shared slice.`);
  if (surface.stateScope !== "global" && scope === "global")
    throw new Error(`Surface ${surface.id} does not keep global state.`);
}

const ISO_DATE = /^\d{4}-\d{2}-\d{2}$/;

/** Records are checked against the schema the manifest declared, in main, so a
 * malformed write never reaches disk and the renderer is not the only guard. */
function checkRecords(surface, value) {
  if (value === null || value === undefined) return;
  const at = `Surface ${surface.id}`;
  if (!Array.isArray(value)) throw new Error(`${at} stores a list of records.`);
  if (value.length > MAX_RECORDS)
    throw new Error(`${at} stores at most ${MAX_RECORDS} records.`);
  const fields = new Map(
    (surface.view?.document?.fields || []).map((field) => [field.id, field]),
  );
  const seen = new Set();
  for (const record of value) {
    if (!record || typeof record !== "object" || Array.isArray(record))
      throw new Error(`${at} stores objects.`);
    if (typeof record.id !== "string" || !record.id || record.id.length > 64)
      throw new Error(`${at}: every record needs an id of up to 64 characters.`);
    if (seen.has(record.id))
      throw new Error(`${at}: record id ${record.id} appears twice.`);
    seen.add(record.id);
    for (const [key, entry] of Object.entries(record)) {
      if (key === "id") continue;
      const field = fields.get(key);
      if (!field)
        throw new Error(`${at}: ${key} is not a field this surface declared.`);
      if (entry === null || entry === undefined) continue;
      if (field.type === "boolean" && typeof entry !== "boolean")
        throw new Error(`${at}: ${key} is true or false.`);
      if (field.type === "text" && typeof entry !== "string")
        throw new Error(`${at}: ${key} is text.`);
      if (field.type === "text" && entry.length > 1000)
        throw new Error(`${at}: ${key} is at most 1000 characters.`);
      if (field.type === "date" && !ISO_DATE.test(String(entry)))
        throw new Error(`${at}: ${key} is a date as YYYY-MM-DD.`);
      if (
        field.type === "select" &&
        !field.options.some((option) => option.value === entry)
      )
        throw new Error(
          `${at}: ${key} must be one of ${field.options.map((option) => option.value).join(", ")}.`,
        );
    }
  }
}

function registerExtensionIpc({
  handle,
  getExtensions,
  getSurfaceState,
  announce = () => {},
}) {
  handle("extensions-state-read", (extensionId, surfaceId, version, scope) => {
    const surface = locate(getExtensions(), extensionId, surfaceId);
    checkAddress(surface, version, scope);
    return getSurfaceState().read(extensionId, surfaceId, version, scope);
  });

  // Reading every project at once is a wider door than reading one, so it
  // opens only for a slice some surface actually declared an aggregate over.
  handle("extensions-state-aggregate", (extensionId, surfaceId, version) => {
    const surface = locate(getExtensions(), extensionId, surfaceId);
    if (version !== surface.stateVersion)
      throw new Error(`Surface ${surfaceId} keeps version ${surface.stateVersion} state.`);
    if (surface.stateScope !== "project")
      throw new Error(`Surface ${surfaceId} does not keep state per project.`);
    if (!getExtensions()?.aggregatesOver(extensionId, surfaceId))
      throw new Error(`No surface aggregates ${extensionId}/${surfaceId}.`);
    return getSurfaceState().aggregate(extensionId, surfaceId, version);
  });

  // Every other view of the same slice - a second pane, an aggregate page -
  // learns that it is now behind.
  handle(
    "extensions-state-write",
    async (extensionId, surfaceId, version, scope, value) => {
      const surface = locate(getExtensions(), extensionId, surfaceId);
      checkAddress(surface, version, scope);
      checkRecords(surface, value);
      await getSurfaceState().write(
        extensionId,
        surfaceId,
        version,
        scope,
        value,
      );
      announce({ extensionId, surfaceId, version, scope });
    },
  );

  handle("extensions-list", () => {
    const extensions = getExtensions();
    if (!extensions) throw new Error("Extension manager is not ready.");
    return extensions.list();
  });

  handle("extensions-refresh", () => {
    const extensions = getExtensions();
    if (!extensions) throw new Error("Extension manager is not ready.");
    return extensions.refresh();
  });

  handle("extensions-set-enabled", (extensionId, enabled) => {
    if (typeof extensionId !== "string" || extensionId.length > 200)
      throw new Error("Invalid extension id.");
    return getExtensions().setEnabled(extensionId, enabled);
  });
}

module.exports = { registerExtensionIpc };
