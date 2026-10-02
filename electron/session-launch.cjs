const path = require("node:path");
const { createHash } = require("node:crypto");
const { appDb, validLaunchRecord } = require("./app-db.cjs");
const { request, errorDetails } = require("./herdr.cjs");
const { quote } = require("./connections.cjs");
const { herdrLaunchParams } = require("./terminal-text.cjs");
const { createWorktree, worktreeBranchError } = require("./worktree.cjs");
const { sessionEnvPrefix, modelLaunch } = require("./project-session.cjs");
const { remoteFileCommand } = require("./ipc/terminals.cjs");

function launchError(code, message) {
  return Object.assign(new Error(message), { code });
}

function preparationSignature(input) {
  return JSON.stringify([
    input.endpoint,
    input.operationId,
    input.kind,
    input.agent,
    input.modelProfileId,
    input.claudeAccountId,
    input.codexAccountId,
    Object.entries(input.env || {}).sort(([a], [b]) => a.localeCompare(b)),
    input.prompt,
  ]);
}

const fingerprint = (value) => createHash("sha256").update(value).digest("hex");

function validate(input) {
  if (!input || typeof input !== "object")
    throw launchError("INVALID_LAUNCH", "Invalid session launch.");
  for (const field of ["operationId", "endpoint", "cwd", "label"])
    if (
      typeof input[field] !== "string" ||
      !input[field] ||
      input[field].length > 4096
    )
      throw launchError("INVALID_LAUNCH", `Invalid launch ${field}.`);
  if (!input.endpoint.startsWith("ssh:") && !path.isAbsolute(input.endpoint))
    throw launchError("INVALID_LAUNCH", "Invalid Herdr endpoint.");
  if (
    !path.isAbsolute(input.cwd) ||
    !["terminal", "agent"].includes(input.kind)
  )
    throw launchError(
      "INVALID_LAUNCH",
      "Choose an absolute project folder and a session kind.",
    );
  if (
    input.kind === "agent" &&
    !["claude", "codex", "gemini", "cursor-agent"].includes(input.agent)
  )
    throw launchError("INVALID_LAUNCH", "Unsupported agent.");
  if (
    input.kind === "agent" &&
    input.modelProfileId &&
    input.agent !== "claude"
  )
    throw launchError(
      "INVALID_LAUNCH",
      "Custom model profiles are supported by Claude sessions.",
    );
  for (const field of [
    "workspaceId",
    "targetPaneId",
    "paneId",
    "modelProfileId",
  ])
    if (
      input[field] !== undefined &&
      (typeof input[field] !== "string" ||
        !input[field] ||
        input[field].length > 200)
    )
      throw launchError("INVALID_LAUNCH", `Invalid launch ${field}.`);
  for (const field of ["claudeAccountId", "codexAccountId"])
    if (
      input[field] !== undefined &&
      (typeof input[field] !== "string" || input[field].length > 200)
    )
      throw launchError("INVALID_LAUNCH", `Invalid launch ${field}.`);
  if (input.worktree) {
    const error = worktreeBranchError(input.worktree.branch);
    if (error) throw launchError("INVALID_LAUNCH", error);
    if (
      input.worktree.base !== undefined &&
      (typeof input.worktree.base !== "string" ||
        !input.worktree.base ||
        input.worktree.base.startsWith("-") ||
        input.worktree.base.length > 1024)
    )
      throw launchError("INVALID_LAUNCH", "Choose an existing base branch.");
  }
  if (input.env !== undefined) {
    if (
      !input.env ||
      typeof input.env !== "object" ||
      Array.isArray(input.env) ||
      Object.keys(input.env).length > 256
    )
      throw launchError("INVALID_LAUNCH", "Invalid session environment.");
    for (const [key, value] of Object.entries(input.env))
      if (
        !/^[A-Za-z_][A-Za-z0-9_]*$/.test(key) ||
        typeof value !== "string" ||
        value.length > 65536 ||
        value.includes("\0")
      )
        throw launchError(
          "INVALID_LAUNCH",
          "Invalid session environment variable.",
        );
  }
  if (
    input.prompt !== undefined &&
    (input.kind !== "agent" ||
      typeof input.prompt !== "string" ||
      !input.prompt.trim() ||
      input.prompt.length > 16384 ||
      // Control characters (a tab included) would act as terminal input, not
      // text. Only a newline is allowed.
      // eslint-disable-next-line no-control-regex
      /[\x00-\x09\x0b-\x1f\x7f-\x9f]/.test(input.prompt))
  )
    throw launchError("INVALID_LAUNCH", "Invalid first prompt.");
}

class SessionLauncher {
  constructor({
    getConnections,
    modelProviders,
    rpc = request,
    readSnapshot,
    checkCompatibility = async () => {},
    worktreeCreate = createWorktree,
    userDataDir,
    savedWorkspaces,
    resolveEnvironment = async () => ({ env: {}, project: null, sends: false }),
    prepareSession,
  }) {
    this.getConnections = getConnections;
    this.modelProviders = modelProviders;
    this.rpc = rpc;
    this.checkCompatibility = checkCompatibility;
    this.readSnapshot = readSnapshot;
    this.worktreeCreate = worktreeCreate;
    this.queues = new Map();
    this.operations = new Map();
    this.projects = new Map();
    this.userDataDir = userDataDir;
    this.savedWorkspaces = savedWorkspaces;
    this.resolveEnvironment = resolveEnvironment;
    this.prepareSession = prepareSession;
    this.journal = new Map();
    this.journalLoaded = false;
  }

  launch(input) {
    try {
      validate(input);
    } catch (error) {
      return Promise.resolve({
        ok: false,
        error: {
          ...errorDetails(error),
          retryable: false,
          stage: "validation",
        },
      });
    }
    const key = JSON.stringify([input.endpoint, input.operationId]);
    const signature = JSON.stringify(input);
    let operation = this.operations.get(key);
    const resumesCreated =
      operation?.created &&
      input.paneId === operation.created.paneId &&
      input.workspaceId === operation.created.workspaceId &&
      input.cwd === operation.created.cwd &&
      preparationSignature(input) === preparationSignature(operation.input);
    if (operation && operation.signature !== signature && !resumesCreated)
      return Promise.resolve({
        ok: false,
        error: {
          code: "OPERATION_CONFLICT",
          message: "This operation ID already belongs to a different launch.",
          retryable: false,
          stage: "validation",
        },
      });
    if (operation?.promise) return operation.promise;
    if (
      operation?.completed &&
      (!this.userDataDir || operation.persistedCompleted)
    )
      return Promise.resolve({ ok: true, value: operation.created });
    if (!operation) {
      operation = { signature, stage: "queued", input };
      this.operations.set(key, operation);
    }
    const previous = this.queues.get(input.endpoint) || Promise.resolve();
    const next = previous.catch(() => {}).then(() => this.execute(operation));
    operation.promise = next;
    this.queues.set(input.endpoint, next);
    next.finally(() => {
      operation.promise = null;
      if (this.queues.get(input.endpoint) === next)
        this.queues.delete(input.endpoint);
    });
    return next;
  }

  async canonical(endpoint, cwd) {
    const result = await this.getConnections().inspect(endpoint, {
      operation: "canonical_checkout",
      root: cwd,
    });
    if (result.missing)
      throw launchError(
        "CHECKOUT_MISSING",
        "The checkout is no longer present on its host.",
      );
    if (typeof result.cwd !== "string" || !path.isAbsolute(result.cwd))
      throw launchError(
        "PREPARATION_FAILED",
        "The host did not report the canonical checkout path.",
      );
    return result.cwd;
  }

  async loadJournal() {
    if (!this.userDataDir || this.journalLoaded) return;
    const rows = appDb(this.userDataDir)
      .prepare("SELECT data FROM launches")
      .all();
    const db = appDb(this.userDataDir);
    for (const row of rows) {
      let record = null;
      try {
        record = JSON.parse(row.data);
      } catch {
        // Falls through to the invalid-row path.
      }
      if (!validLaunchRecord(record)) {
        console.warn("Dropping an invalid saved session launch record.");
        db.prepare("DELETE FROM launches WHERE data = ?").run(row.data);
        continue;
      }
      this.journal.set(
        JSON.stringify([record.endpoint, record.operationId]),
        record,
      );
    }
    this.journalLoaded = true;
  }

  async persist(operation) {
    if (!this.userDataDir) return;
    const input = operation.input;
    const record = {
      endpoint: input.endpoint,
      operationId: input.operationId,
      signatureHash: fingerprint(operation.signature),
      preparationHash: fingerprint(preparationSignature(input)),
      created: operation.created,
      sourceCwd: operation.sourceCwd || input.cwd,
      completed: !!operation.completed,
    };
    this.journal.set(
      JSON.stringify([input.endpoint, input.operationId]),
      record,
    );
    appDb(this.userDataDir)
      .prepare(
        "INSERT INTO launches(endpoint, operation_id, data) VALUES(?, ?, ?) " +
          "ON CONFLICT(endpoint, operation_id) DO UPDATE SET data = excluded.data",
      )
      .run(record.endpoint, record.operationId, JSON.stringify(record));
    if (operation.completed) operation.persistedCompleted = true;
  }

  async prepare(input, environment, resolved) {
    if (this.prepareSession)
      return this.prepareSession(input, { ...environment, model: resolved });
    if (
      input.codexAccountId ||
      (input.claudeAccountId && !this.modelProviders?.resolveClaudeAccount)
    )
      throw launchError(
        "SESSION_PREPARATION_FAILED",
        "The selected account cannot be prepared.",
      );
    const connections = this.getConnections();
    return sessionEnvPrefix(
      {
        connections,
        resolveAccount: this.modelProviders?.resolveClaudeAccount?.bind(
          this.modelProviders,
        ),
        resolveModel: async () => resolved,
        upload: async (endpoint, payload) =>
          (
            await connections.exec(endpoint, remoteFileCommand(), {
              input: payload,
            })
          ).trim(),
        remove: (endpoint, file) =>
          connections.exec(endpoint, `rm -f ${quote(file)}`).catch(() => {}),
      },
      {
        ...input,
        agent: input.kind === "agent" ? input.agent : undefined,
        nativeEnvironment: environment,
      },
    );
  }

  async findWorkspace(endpoint, cwd, snapshot) {
    const saved = this.savedWorkspaces ? this.savedWorkspaces() : [];
    const candidates = snapshot.workspaces.map((workspace) => ({
      workspace,
      pane: snapshot.panes.find(
        (pane) => pane.workspace_id === workspace.workspace_id,
      ),
    }));
    for (const { workspace, pane } of candidates) {
      const retained = [...this.journal.values()].find(
        (record) =>
          record.endpoint === endpoint &&
          record.created.workspaceId === workspace.workspace_id,
      );
      const savedWorkspace = saved.find(
        (item) =>
          item.connection === endpoint &&
          item.herdrId === workspace.workspace_id,
      );
      const candidatePath =
        workspace.worktree?.checkout_path ||
        savedWorkspace?.cwd ||
        retained?.created.cwd ||
        pane?.cwd;
      if (!candidatePath) continue;
      let canonical;
      try {
        canonical = await this.canonical(endpoint, candidatePath);
      } catch (error) {
        if (error.code === "CHECKOUT_MISSING") continue;
        throw error;
      }
      if (canonical === cwd && pane)
        return { workspaceId: workspace.workspace_id, paneId: pane.pane_id };
    }
    return null;
  }

  async snapshot(endpoint, socket) {
    const response = this.readSnapshot
      ? await this.readSnapshot(endpoint)
      : await this.rpc(socket, "session.snapshot", {});
    return response.snapshot || response;
  }

  async execute(operation) {
    const input = operation.input;
    try {
      operation.stage = "journal";
      await this.loadJournal();
      const saved = this.journal.get(
        JSON.stringify([input.endpoint, input.operationId]),
      );
      if (saved && !operation.created) {
        const resume =
          input.paneId === saved.created.paneId &&
          input.workspaceId === saved.created.workspaceId &&
          input.cwd === saved.created.cwd &&
          fingerprint(preparationSignature(input)) === saved.preparationHash;
        if (fingerprint(operation.signature) !== saved.signatureHash && !resume)
          throw launchError(
            "OPERATION_CONFLICT",
            "This operation ID already belongs to a different launch.",
          );
        operation.created = saved.created;
        operation.sourceCwd = saved.sourceCwd || input.cwd;
        operation.completed = saved.completed;
      }
      operation.stage = "compatibility";
      await this.checkCompatibility(input.endpoint);
      const socket = await this.getConnections().socket(input.endpoint);
      if (saved) {
        const snapshot = await this.snapshot(input.endpoint, socket);
        if (
          !snapshot.panes.some(
            (pane) =>
              pane.pane_id === saved.created.paneId &&
              pane.workspace_id === saved.created.workspaceId,
          )
        )
          throw launchError(
            "SESSION_MISSING",
            "The recorded session is no longer available. Start a new launch to create another session.",
          );
        if (operation.completed) {
          await this.persist(operation);
          return { ok: true, value: operation.created };
        }
      }
      operation.stage = "environment";
      const environment = await this.resolveEnvironment({
        ...input,
        cwd: operation.sourceCwd || input.cwd,
      });
      const resolved = input.modelProfileId
        ? await this.modelProviders.resolveEnv(input.modelProfileId)
        : null;
      const env = {
        ...environment.env,
        ...input.env,
        ...(resolved?.settings || {}),
      };
      if (!operation.created) {
        operation.stage = "checkout";
        operation.cwd ||= await this.canonical(input.endpoint, input.cwd);
        if (input.worktree && !operation.worktreePath) {
          const prepared = await this.getConnections().inspect(input.endpoint, {
            operation: "worktree_base",
            root: operation.cwd,
            base: input.worktree.base,
          });
          if (
            typeof prepared.cwd !== "string" ||
            !path.isAbsolute(prepared.cwd) ||
            typeof prepared.base !== "string" ||
            !prepared.base
          )
            throw launchError(
              "SESSION_PREPARATION_FAILED",
              "The host did not report the prepared worktree base.",
            );
          const created = input.endpoint.startsWith("ssh:")
            ? await this.getConnections().inspect(input.endpoint, {
                operation: "worktree_create",
                root: prepared.cwd,
                branch: input.worktree.branch,
                base: prepared.base,
              })
            : await this.worktreeCreate(
                prepared.cwd,
                input.worktree.branch,
                prepared.base,
              );
          operation.worktreePath = created.path;
          operation.cwd = await this.canonical(input.endpoint, created.path);
        }
        operation.stage = "creation";
        if (input.paneId) {
          const snapshot = await this.snapshot(input.endpoint, socket);
          if (
            !snapshot.panes.some(
              (pane) =>
                pane.pane_id === input.paneId &&
                pane.workspace_id === input.workspaceId,
            )
          )
            throw launchError(
              "SESSION_MISSING",
              "The session to resume is no longer available.",
            );
          operation.created = {
            operationId: input.operationId,
            workspaceId: input.workspaceId,
            paneId: input.paneId,
            cwd: operation.cwd,
            createdWorkspace: false,
          };
        } else {
          const projectKey = JSON.stringify([input.endpoint, operation.cwd]);
          let existing =
            !input.worktree && input.workspaceId
              ? { workspaceId: input.workspaceId, paneId: input.targetPaneId }
              : !input.worktree
                ? this.projects.get(projectKey)
                : null;
          if (!existing && !input.worktree) {
            const snapshot = await this.snapshot(input.endpoint, socket);
            existing = await this.findWorkspace(
              input.endpoint,
              operation.cwd,
              snapshot,
            );
          }
          let result;
          if (existing) {
            try {
              result = await this.rpc(
                socket,
                "pane.split",
                herdrLaunchParams("pane.split", {
                  workspace_id: existing.workspaceId,
                  target_pane_id: existing.paneId,
                  direction: "right",
                  focus: false,
                  cwd: operation.cwd,
                  env,
                }),
              );
            } catch (error) {
              if (
                !["workspace_not_found", "pane_not_found"].includes(error.code)
              )
                throw error;
              this.projects.delete(projectKey);
              existing = await this.findWorkspace(
                input.endpoint,
                operation.cwd,
                await this.snapshot(input.endpoint, socket),
              );
              if (existing)
                result = await this.rpc(
                  socket,
                  "pane.split",
                  herdrLaunchParams("pane.split", {
                    workspace_id: existing.workspaceId,
                    target_pane_id: existing.paneId,
                    direction: "right",
                    focus: false,
                    cwd: operation.cwd,
                    env,
                  }),
                );
            }
          }
          if (!existing)
            result = await this.rpc(
              socket,
              "workspace.create",
              herdrLaunchParams("workspace.create", {
                label: input.label,
                cwd: operation.cwd,
                focus: false,
                env,
              }),
            );
          const workspaceId =
            existing?.workspaceId || result.workspace?.workspace_id;
          const paneId = existing
            ? result.pane?.pane_id || result.pane_id
            : result.root_pane?.pane_id;
          if (!workspaceId || !paneId)
            throw launchError(
              "INVALID_CREATION_RESPONSE",
              "Herdr did not report the created session IDs.",
            );
          operation.created = {
            operationId: input.operationId,
            workspaceId,
            paneId,
            cwd: operation.cwd,
            createdWorkspace: !existing,
          };
          if (!input.worktree)
            this.projects.set(projectKey, { workspaceId, paneId });
        }
      }
      operation.stage = "journal";
      await this.persist(operation);
      operation.stage = "agent";
      if (input.kind === "agent") {
        const { prefix, settings, launch } = await this.prepare(
          { ...input, cwd: operation.created.cwd },
          environment,
          resolved,
        );
        const command =
          launch || (resolved ? modelLaunch(resolved.settings) : input.agent);
        const prompt = input.prompt ? ` ${quote(input.prompt)}` : "";
        const text = prefix
          ? `(${prefix}exec ${command}${settings}${prompt})`
          : `${command}${settings}${prompt}`;
        await this.rpc(socket, "pane.send_input", {
          pane_id: operation.created.paneId,
          text,
          keys: ["Enter"],
        });
      }
      operation.completed = true;
      operation.stage = "journal";
      await this.persist(operation);
      return { ok: true, value: operation.created };
    } catch (error) {
      return {
        ok: false,
        error: {
          ...errorDetails(error),
          code:
            error.code ||
            (["environment", "checkout", "agent"].includes(operation.stage)
              ? "SESSION_PREPARATION_FAILED"
              : "HERDR_ERROR"),
          retryable: ![
            "INVALID_LAUNCH",
            "OPERATION_CONFLICT",
            "HERDR_INCOMPATIBLE",
            "SESSION_MISSING",
          ].includes(error.code),
          stage: operation.stage,
          ...(operation.created ? { created: operation.created } : {}),
        },
      };
    }
  }
}

function registerSessionLaunchIpc({ handle, ...options }) {
  const launcher = new SessionLauncher(options);
  handle("session-launch", (input) => launcher.launch(input));
  return launcher;
}

module.exports = { SessionLauncher, registerSessionLaunchIpc };
