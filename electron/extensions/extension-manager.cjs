const path = require("node:path");
const { randomUUID } = require("node:crypto");
const { mkdir, readFile, rename, rm, writeFile } = require("node:fs/promises");
const { validateExtensionManifest } = require("./manifest.cjs");
const { scanLocalExtensions } = require("./local-extensions.cjs");

const SCHEMA_VERSION = 2;

function defaultState(manifests = [], externalEnabled = false) {
  return {
    schemaVersion: SCHEMA_VERSION,
    revision: 0,
    extensions: Object.fromEntries(
      manifests.map((manifest) => [
        manifest.id,
        {
          enabled: manifest.source.kind === "builtin" || externalEnabled,
          overrides: {},
        },
      ]),
    ),
  };
}

function defaultLock() {
  return { schemaVersion: SCHEMA_VERSION, packages: {} };
}

async function readJson(file) {
  try {
    return { exists: true, value: JSON.parse(await readFile(file, "utf8")) };
  } catch (error) {
    if (error?.code === "ENOENT") return { exists: false, value: null };
    return { exists: true, value: null, error };
  }
}

async function writeJsonAtomic(file, value) {
  await mkdir(path.dirname(file), { recursive: true, mode: 0o700 });
  const temporary = `${file}.${randomUUID()}.tmp`;
  await writeFile(temporary, `${JSON.stringify(value, null, 2)}\n`, {
    mode: 0o600,
    flag: "wx",
  });
  try {
    await rename(temporary, file);
  } catch (error) {
    await rm(temporary, { force: true }).catch(() => {});
    throw error;
  }
}

function validState(value) {
  return Boolean(
    value &&
    typeof value === "object" &&
    value.schemaVersion === SCHEMA_VERSION &&
    (value.revision === undefined ||
      (Number.isInteger(value.revision) && value.revision >= 0)) &&
    value.extensions &&
    typeof value.extensions === "object" &&
    !Array.isArray(value.extensions) &&
    Object.values(value.extensions).every(
      (extension) =>
        extension &&
        typeof extension === "object" &&
        !Array.isArray(extension) &&
        typeof extension.enabled === "boolean" &&
        extension.overrides &&
        typeof extension.overrides === "object" &&
        !Array.isArray(extension.overrides),
    ),
  );
}

function validLock(value) {
  return Boolean(
    value &&
    typeof value === "object" &&
    value.schemaVersion === SCHEMA_VERSION &&
    value.packages &&
    typeof value.packages === "object" &&
    !Array.isArray(value.packages),
  );
}

function lockMatchesManifest(lockEntry, manifest) {
  return Boolean(
    lockEntry &&
    lockEntry.version === manifest.version &&
    lockEntry.apiVersion === manifest.apiVersion &&
    JSON.stringify(lockEntry.source) === JSON.stringify(manifest.source),
  );
}

class ExtensionManager {
  constructor({ dataDir, builtins = [], installed = [], localDir }) {
    if (typeof dataDir !== "string" || !path.isAbsolute(dataDir))
      throw new Error("Extension data directory must be absolute.");
    this.dataDir = dataDir;
    this.extensionDir = path.join(dataDir, "extensions");
    this.stateFile = path.join(this.extensionDir, "extensions.json");
    this.lockFile = path.join(this.extensionDir, "extension-lock.json");
    this.localDir = localDir;
    this.problems = [];
    // Built-ins and installed packages come from code, so a bad one is a bug
    // and throws. Folders the user edits are validated per entry instead.
    this.static = new Map();
    for (const manifest of [...builtins, ...installed]) {
      const normalized = validateExtensionManifest(manifest);
      if (this.static.has(normalized.id))
        throw new Error(`Duplicate extension: ${normalized.id}.`);
      if (builtins.includes(manifest) && normalized.source.kind !== "builtin")
        throw new Error("Built-in extension must use the builtin source.");
      if (installed.includes(manifest) && normalized.source.kind === "builtin")
        throw new Error(
          "Installed user extension must not use the builtin source.",
        );
      this.static.set(normalized.id, normalized);
    }
    this.manifests = new Map(this.static);
    this.state = defaultState([...this.manifests.values()]);
    this.lock = defaultLock();
    this.diagnostic = undefined;
    this.revision = 0;
    // A failed init must not surface as an unhandled rejection: the manager is
    // built at main.cjs module scope, long before anything awaits `ready`.
    this.ready = this.init().catch((error) => {
      this.diagnostic = `Extension settings could not be initialized: ${error?.message || error}`;
      for (const manifest of this.manifests.values())
        this.state.extensions[manifest.id] = {
          enabled: manifest.source.kind === "builtin",
          overrides: {},
        };
    });
  }

  async init() {
    await this.reload();
    await this.reconcile();
  }

  /** Re-reads the local extensions folder. The manifest map is replaced in one
   * assignment, so nothing ever observes a half-scanned folder. */
  async reload() {
    if (!this.localDir) {
      this.manifests = new Map(this.static);
      this.problems = [];
      return;
    }
    const { manifests, problems } = await scanLocalExtensions(this.localDir, [
      ...this.static.keys(),
    ]);
    this.manifests = new Map([...this.static, ...manifests]);
    this.problems = problems;
  }

  /** Rescans the folder and re-syncs settings without restarting the app. */
  async refresh() {
    this.ready = this.ready.then(() =>
      this.reload().then(() => this.reconcile()),
    );
    await this.ready;
    return this.snapshot();
  }

  async reconcile() {
    let stateWritable = true;
    let stateChanged = false;
    const stateFile = await readJson(this.stateFile);
    if (!stateFile.exists) {
      this.state = defaultState([...this.manifests.values()]);
    } else if (validState(stateFile.value)) {
      this.state = stateFile.value;
      this.revision = Number.isInteger(this.state.revision)
        ? this.state.revision
        : 0;
    } else {
      stateWritable = false;
      this.state = defaultState([...this.manifests.values()], false);
      this.diagnostic =
        "Extension settings were damaged or created by a newer version; external extensions were disabled.";
    }

    let lockWritable = true;
    const lockFile = await readJson(this.lockFile);
    if (validLock(lockFile.value)) this.lock = lockFile.value;
    else if (lockFile.exists) {
      lockWritable = false;
      this.lock = defaultLock();
      this.diagnostic =
        this.diagnostic ||
        "Extension lock metadata is invalid; external extensions were disabled.";
      for (const manifest of this.manifests.values())
        if (manifest.source.kind !== "builtin")
          this.state.extensions[manifest.id] = {
            enabled: false,
            overrides: {},
          };
    }
    let lockChanged = false;
    for (const manifest of this.manifests.values()) {
      // A local folder has no package identity to pin: bumping the version in
      // your own manifest is the normal edit loop, not a supply-chain event.
      if (manifest.source.kind === "local") {
        if (!this.state.extensions[manifest.id]) {
          this.state.extensions[manifest.id] = { enabled: false, overrides: {} };
          stateChanged = true;
        }
        continue;
      }
      const lockEntry = this.lock.packages[manifest.id];
      if (!lockEntry) {
        this.lock.packages[manifest.id] = {
          source: manifest.source,
          version: manifest.version,
          apiVersion: manifest.apiVersion,
        };
        lockChanged = true;
      } else if (
        manifest.source.kind !== "builtin" &&
        !lockMatchesManifest(lockEntry, manifest)
      ) {
        lockWritable = false;
        this.diagnostic =
          this.diagnostic ||
          `Extension lock entry for ${manifest.id} does not match its manifest; the external extension was disabled.`;
        if (this.state.extensions[manifest.id]?.enabled !== false) {
          this.state.extensions[manifest.id] = {
            ...(this.state.extensions[manifest.id] || { overrides: {} }),
            enabled: false,
          };
          stateChanged = true;
        }
      }
      if (!this.state.extensions[manifest.id]) {
        this.state.extensions[manifest.id] = {
          enabled: manifest.source.kind === "builtin",
          overrides: {},
        };
        stateChanged = true;
      }
      if (
        manifest.source.kind === "builtin" &&
        this.state.extensions[manifest.id].enabled !== true
      ) {
        this.state.extensions[manifest.id] = {
          ...this.state.extensions[manifest.id],
          enabled: true,
        };
        stateChanged = true;
      }
    }
    // Existing resolved entries are never replaced from a manifest. This is
    // what keeps a lock commit stable across restarts.
    if (lockWritable && (lockChanged || !lockFile.exists))
      await writeJsonAtomic(this.lockFile, this.lock);
    if (stateWritable && (stateChanged || !stateFile.exists))
      await writeJsonAtomic(this.stateFile, this.state);
  }

  /** The active surface behind an address, or undefined. Synchronous so IPC
   * can check a request before touching the store. */
  /** Whether any enabled surface of this extension reads the named slice as an
   * aggregate. The address the renderer sends is a state id, not the id of the
   * surface asking, so this is the only way to know the door was opened. */
  aggregatesOver(extensionId, stateId) {
    if (!this.state.extensions[extensionId]?.enabled) return false;
    return (
      this.manifests
        .get(extensionId)
        ?.contributions.surfaces.some(
          (surface) => surface.aggregate && surface.stateId === stateId,
        ) === true
    );
  }

  activeSurface(extensionId, surfaceId) {
    if (!this.state.extensions[extensionId]?.enabled) return undefined;
    return this.manifests
      .get(extensionId)
      ?.contributions.surfaces.find((surface) => surface.id === surfaceId);
  }

  async snapshot() {
    await this.ready;
    const records = [...this.manifests.values()].map((manifest) => {
      const state = this.state.extensions[manifest.id] || { enabled: false };
      const enabled = Boolean(state.enabled);
      return {
        manifest,
        status: enabled ? "active" : "disabled",
        ...(this.diagnostic && !enabled ? { error: this.diagnostic } : {}),
      };
    });
    return {
      schemaVersion: SCHEMA_VERSION,
      version: this.revision,
      ...(this.diagnostic ? { diagnostic: this.diagnostic } : {}),
      ...(this.localDir ? { localDir: this.localDir } : {}),
      problems: this.problems,
      extensions: records,
      surfaces: records.flatMap(
        (record) => record.manifest.contributions.surfaces,
      ),
      navigation: records.flatMap(
        (record) => record.manifest.contributions.navigation,
      ),
      actions: records.flatMap(
        (record) => record.manifest.contributions.actions,
      ),
      commands: records.flatMap(
        (record) => record.manifest.contributions.commands,
      ),
    };
  }

  async list() {
    return this.snapshot();
  }

  async setEnabled(extensionId, enabled) {
    await this.ready;
    if (!this.manifests.has(extensionId))
      throw new Error(`Unknown extension: ${extensionId}.`);
    if (typeof enabled !== "boolean")
      throw new Error("Extension enabled state must be boolean.");
    const manifest = this.manifests.get(extensionId);
    if (manifest.source.kind === "builtin" && !enabled)
      throw new Error("Built-in extensions cannot be disabled.");
    if (this.diagnostic && manifest.source.kind !== "builtin" && enabled)
      throw new Error(
        "External extensions are disabled until extension settings are repaired.",
      );
    const current = this.state.extensions[extensionId]?.enabled === true;
    if (current === enabled) return this.snapshot();
    this.state.extensions[extensionId] = {
      ...(this.state.extensions[extensionId] || { overrides: {} }),
      enabled,
    };
    this.revision += 1;
    this.state.revision = this.revision;
    await writeJsonAtomic(this.stateFile, this.state);
    return this.snapshot();
  }
}

module.exports = {
  ExtensionManager,
  SCHEMA_VERSION,
  defaultState,
  defaultLock,
  readJson,
  writeJsonAtomic,
};
