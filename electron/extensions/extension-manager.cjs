const path = require("node:path");
const { DAMAGED, readStore, writeStore } = require("../app-db.cjs");
const { validateExtensionManifest } = require("./manifest.cjs");
const { scanLocalExtensions } = require("./local-extensions.cjs");
const { listSshHosts } = require("./hosts.cjs");

const SCHEMA_VERSION = 2;

/** Built-ins the owner may switch off. Every other built-in is part of the
 * shell and is forced back on at start. */
const DISABLEABLE_BUILTINS = new Set([
  "builtin.orchestrator",
  "builtin.artifacts",
]);

/** Built-ins that start switched off: opting in is the owner's act. */
const DEFAULT_OFF_BUILTINS = new Set(["builtin.orchestrator"]);

function defaultEnabled(manifest, externalEnabled = false) {
  return manifest.source.kind === "builtin"
    ? !DEFAULT_OFF_BUILTINS.has(manifest.id)
    : externalEnabled;
}

function defaultState(manifests = [], externalEnabled = false) {
  return {
    schemaVersion: SCHEMA_VERSION,
    revision: 0,
    extensions: Object.fromEntries(
      manifests.map((manifest) => [
        manifest.id,
        {
          enabled: defaultEnabled(manifest, externalEnabled),
          overrides: {},
        },
      ]),
    ),
  };
}

function canDisable(manifest) {
  return (
    manifest.source.kind !== "builtin" || DISABLEABLE_BUILTINS.has(manifest.id)
  );
}

function defaultLock() {
  return { schemaVersion: SCHEMA_VERSION, packages: {} };
}

/** A document of sushiai.db (store `name`, key `value`): whether it exists
 * and what it holds. */
function readDoc(dataDir, name) {
  const store = readStore(dataDir, name, { damaged: true });
  if (!Object.hasOwn(store, "value")) return { exists: false, value: null };
  // Present but unreadable: fail closed, never start over from defaults.
  if (store.value === DAMAGED) return { exists: true, value: null };
  return { exists: true, value: store.value };
}

function writeDoc(dataDir, name, value) {
  writeStore(dataDir, name, { value });
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
  /** `companions` is a createCompanions() supervisor (companion-process.cjs).
   * Without one, a manifest's companion block is listed but never started. */
  constructor({
    dataDir,
    builtins = [],
    installed = [],
    localDir,
    companions,
  }) {
    if (typeof dataDir !== "string" || !path.isAbsolute(dataDir))
      throw new Error("Extension data directory must be absolute.");
    this.dataDir = dataDir;
    this.extensionDir = path.join(dataDir, "extensions");
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
    this.listeners = new Set();
    this.companions = companions;
    this.companionIds = new Set();
    this.companionListeners = new Set();
    // A companion says a view changed: tell whoever draws it.
    this.companions?.subscribe((event) => {
      const surfaceIds =
        event.type === "changed"
          ? [event.surfaceId]
          : this.companionSurfaces(event.extensionId);
      for (const surfaceId of surfaceIds)
        for (const listener of this.companionListeners)
          listener({ extensionId: event.extensionId, surfaceId });
    });
    // A failed init must not surface as an unhandled rejection: the manager is
    // built at main.cjs module scope, long before anything awaits `ready`.
    this.ready = this.init().catch((error) => {
      this.diagnostic = `Extension settings could not be initialized: ${error?.message || error}`;
      for (const manifest of this.manifests.values())
        this.state.extensions[manifest.id] = {
          enabled: defaultEnabled(manifest),
          overrides: {},
        };
    });
  }

  async init() {
    await this.reload();
    await this.reconcile();
    await this.syncCompanions();
  }

  companionSurfaces(extensionId) {
    return (
      this.manifests
        .get(extensionId)
        ?.contributions.surfaces.filter(
          (surface) => surface.view.kind === "companion",
        )
        .map((surface) => surface.id) ?? []
    );
  }

  /** Brings every companion process in line with the enabled state and the
   * recorded approval. A manifest that went away loses its process. */
  async syncCompanions(only) {
    if (!this.companions) return;
    const ids = only ? [only] : [...this.manifests.keys()];
    for (const id of only ? [] : this.companionIds)
      if (!this.manifests.has(id)) {
        this.companionIds.delete(id);
        await this.companions.remove(id);
      }
    for (const id of ids) {
      const manifest = this.manifests.get(id);
      if (!manifest?.companion) continue;
      this.companionIds.add(id);
      await this.companions.sync(id, {
        manifest,
        enabled: this.isEnabled(id),
        approved: this.state.extensions[id]?.approved,
        extensionDir:
          manifest.source.kind === "local" ? manifest.source.path : undefined,
      });
    }
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
      this.reload()
        .then(() => this.reconcile())
        .then(() => this.syncCompanions()),
    );
    await this.ready;
    return this.snapshot();
  }

  async reconcile() {
    let stateWritable = true;
    let stateChanged = false;
    const stateFile = readDoc(this.dataDir, "extensions");
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
    const lockFile = readDoc(this.dataDir, "extension-lock");
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
    // A built-in the app no longer ships leaves its saved settings and lock
    // entry behind: they are dropped, never shown and never an error.
    const retired = (id) =>
      id.startsWith("builtin.") && !this.manifests.has(id);
    for (const id of Object.keys(this.state.extensions))
      if (retired(id)) {
        delete this.state.extensions[id];
        stateChanged = true;
      }
    for (const id of Object.keys(this.lock.packages))
      if (retired(id)) {
        delete this.lock.packages[id];
        lockChanged = true;
      }
    for (const manifest of this.manifests.values()) {
      // A local folder has no package identity to pin: bumping the version in
      // your own manifest is the normal edit loop, not a supply-chain event.
      if (manifest.source.kind === "local") {
        if (!this.state.extensions[manifest.id]) {
          this.state.extensions[manifest.id] = {
            enabled: false,
            overrides: {},
          };
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
          enabled: defaultEnabled(manifest),
          overrides: {},
        };
        stateChanged = true;
      }
      if (
        manifest.source.kind === "builtin" &&
        !canDisable(manifest) &&
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
      writeDoc(this.dataDir, "extension-lock", this.lock);
    if (stateWritable && (stateChanged || !stateFile.exists))
      writeDoc(this.dataDir, "extensions", this.state);
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

  /** Synchronous, for main-process gating; false until settings are read. */
  isEnabled(extensionId) {
    return this.state.extensions[extensionId]?.enabled === true;
  }

  /** Calls `listener(extensionId, enabled)` after a state change is saved. */
  onChange(listener) {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  async snapshot() {
    await this.ready;
    const records = [...this.manifests.values()].map((manifest) => {
      const state = this.state.extensions[manifest.id] || { enabled: false };
      const enabled = Boolean(state.enabled);
      return {
        manifest,
        status: enabled ? "active" : "disabled",
        canDisable: canDisable(manifest),
        ...(manifest.companion
          ? { companion: this.companionStatus(manifest) }
          : {}),
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

  companionStatus(manifest) {
    return (
      this.companions?.status(manifest.id) ?? {
        state: "off",
        args: [...manifest.companion.args],
        permissions: [...manifest.companion.permissions],
      }
    );
  }

  /** Records the owner's consent to run this extension's companion: the
   * resolved path, args and permissions as they are now. A later change to any
   * of the three asks again. There is no binary hash, so an update in place
   * does not. */
  async approve(extensionId) {
    await this.ready;
    const manifest = this.manifests.get(extensionId);
    if (!manifest?.companion)
      throw new Error(`Extension ${extensionId} declares no companion.`);
    if (this.diagnostic)
      throw new Error(
        "External extensions are disabled until extension settings are repaired.",
      );
    const bound = this.companions?.describe(extensionId);
    if (!bound?.resolvedPath)
      throw new Error("The companion command cannot be resolved.");
    this.state.extensions[extensionId] = {
      ...(this.state.extensions[extensionId] || {
        enabled: false,
        overrides: {},
      }),
      approved: {
        path: bound.resolvedPath,
        args: [...bound.args],
        permissions: [...bound.permissions],
      },
    };
    this.revision += 1;
    this.state.revision = this.revision;
    writeDoc(this.dataDir, "extensions", this.state);
    await this.syncCompanions(extensionId);
  }

  /** The companion view an enabled extension declared at this address. */
  companionSurface(extensionId, surfaceId) {
    const surface = this.activeSurface(extensionId, surfaceId);
    if (!surface || surface.view.kind !== "companion")
      throw new Error(
        `No active companion view at ${extensionId}/${surfaceId}.`,
      );
    return surface;
  }

  async companionRead(extensionId, surfaceId) {
    await this.ready;
    const surface = this.companionSurface(extensionId, surfaceId);
    return this.companions.read(extensionId, surfaceId, surface.view);
  }

  /** Runs one declared action. The renderer names only the action id: the
   * method and what the host adds to the call come from the manifest. */
  async companionAction(extensionId, surfaceId, actionId) {
    await this.ready;
    const surface = this.companionSurface(extensionId, surfaceId);
    const action = surface.view.actions.find((item) => item.id === actionId);
    if (!action) throw new Error(`Unknown companion action: ${actionId}.`);
    const params = { surfaceId };
    if (action.send?.includes("hosts"))
      params.hosts = listSshHosts(() => this.companions.hosts());
    return this.companions.call(
      extensionId,
      action.method,
      params,
      surface.view,
    );
  }

  /** listener({extensionId, surfaceId}) when a companion view has new values
   * or its process changed state. Returns an unsubscribe. */
  onCompanionChanged(listener) {
    this.companionListeners.add(listener);
    return () => this.companionListeners.delete(listener);
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
    if (!enabled && !canDisable(manifest))
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
    writeDoc(this.dataDir, "extensions", this.state);
    for (const listener of this.listeners) listener(extensionId, enabled);
    await this.syncCompanions(extensionId);
    return this.snapshot();
  }
}

module.exports = {
  ExtensionManager,
  SCHEMA_VERSION,
  defaultState,
  defaultLock,
};
