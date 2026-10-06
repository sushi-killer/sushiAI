"use strict";

// Session launch through the sushiai daemon. The renderer sends one
// DaemonLaunchRequest per launch (one idempotencyKey, reused on retry); this
// module validates it, resolves env and accounts, prepares the checkout (and a
// worktree when asked) and sends `session.create`. Nothing is typed into a
// shell and no env file is written: every variable travels in the request.

const fs = require("node:fs/promises");
const path = require("node:path");
const { createWorktree, worktreeBranchError } = require("./worktree.cjs");
const { sessionLaunchEnv } = require("./project-session.cjs");

const SHELL_CMD = ["/bin/sh", "-lc", 'exec "${SHELL:-/bin/sh}" -l'];
const NATIVE_AGENTS = ["claude", "codex"];
const CMD_AGENTS = ["gemini", "cursor-agent"];

function launchError(code, message) {
  return Object.assign(new Error(message), { code });
}

function validate(request) {
  if (!request || typeof request !== "object")
    throw launchError("INVALID_LAUNCH", "Invalid session launch.");
  if (request.host !== "local")
    throw launchError(
      "REMOTE_LAUNCH_LATER",
      "Remote launch comes later. Start sessions on this Mac for now.",
    );
  if (typeof request.cwd !== "string" || !path.isAbsolute(request.cwd))
    throw launchError("INVALID_LAUNCH", "Choose an absolute project folder.");
  if (
    typeof request.idempotencyKey !== "string" ||
    !request.idempotencyKey ||
    request.idempotencyKey.length > 256
  )
    throw launchError("INVALID_LAUNCH", "Invalid launch key.");
  const { agent } = request;
  if (
    agent !== undefined &&
    !NATIVE_AGENTS.includes(agent) &&
    !CMD_AGENTS.includes(agent)
  )
    throw launchError("INVALID_LAUNCH", "Unsupported agent.");
  if (request.modelProfileId && agent !== "claude")
    throw launchError(
      "INVALID_LAUNCH",
      "Custom model profiles are supported by Claude sessions.",
    );
  if (request.worktree) {
    const error = worktreeBranchError(request.worktree.branch);
    if (error) throw launchError("INVALID_LAUNCH", error);
    const base = request.worktree.base;
    if (
      base !== undefined &&
      (typeof base !== "string" ||
        !base ||
        base.startsWith("-") ||
        base.length > 1024)
    )
      throw launchError("INVALID_LAUNCH", "Choose an existing base branch.");
  }
}

function createSessionLauncher({
  manager,
  environment,
  worktreeCreate = createWorktree,
}) {
  // A retry with the same key reuses its worktree instead of failing on it.
  const worktrees = new Map();

  async function checkout(request) {
    let cwd;
    try {
      cwd = await fs.realpath(request.cwd);
    } catch {
      throw launchError(
        "CHECKOUT_MISSING",
        "The checkout is no longer present on this Mac.",
      );
    }
    if (!request.worktree) return cwd;
    let made = worktrees.get(request.idempotencyKey);
    if (!made) {
      made = await worktreeCreate(
        cwd,
        request.worktree.branch,
        request.worktree.base,
      );
      worktrees.set(request.idempotencyKey, made);
    }
    return fs.realpath(made.path);
  }

  async function launch(request) {
    validate(request);
    const cwd = await checkout(request);
    const { env, claudeSettings, project } = await environment({
      host: request.host,
      cwd: request.cwd,
      agent: request.agent,
      claudeAccountId: request.claudeAccountId,
      codexAccountId: request.codexAccountId,
      modelProfileId: request.modelProfileId,
    });
    const native = NATIVE_AGENTS.includes(request.agent);
    const params = {
      cwd,
      cols: request.cols,
      rows: request.rows,
      env,
      idempotencyKey: request.idempotencyKey,
    };
    if (!request.agent) params.cmd = SHELL_CMD;
    else if (!native) params.cmd = [request.agent];
    if (request.agent) params.agent = request.agent;
    for (const key of ["title", "model", "prompt", "resume"])
      if (request[key]) params[key] = request[key];
    if (request.extraArgs?.length) params.extraArgs = request.extraArgs;
    const projectId = request.project || project?.id;
    if (projectId) params.project = projectId;
    if (request.group) params.group = request.group;
    if (request.agent === "claude" && Object.keys(claudeSettings).length)
      params.claudeSettings = claudeSettings;
    const result = await manager.request("local", "session.create", params);
    const sessionId = result?.id ?? result?.sessionId;
    if (typeof sessionId !== "string" || !sessionId)
      throw launchError(
        "INVALID_CREATION_RESPONSE",
        "The daemon did not report the new session.",
      );
    return { host: request.host, sessionId };
  }

  return { launch };
}

/** The launcher the IPC handler uses: accounts resolve through the app's
 * stores. `deps` carries projects, connections, modelProviders, codexAccounts. */
function createDaemonLaunch({
  manager,
  projects,
  connections,
  modelProviders,
  codexAccounts,
}) {
  return createSessionLauncher({
    manager,
    environment: (input) =>
      sessionLaunchEnv(
        {
          projects,
          connections,
          resolveAccount: (id) => modelProviders.resolveClaudeAccount(id),
          resolveCodexAccount: (id, endpoint) =>
            codexAccounts.resolve(id, endpoint),
          resolveModel: (id) => modelProviders.resolveEnv(id),
        },
        input,
      ),
  });
}

// The Herdr-era `session-launch` channel is gone; main.cjs (owned by another
// lane) still calls this and must drop the call at merge.
function registerSessionLaunchIpc() {}

module.exports = {
  createSessionLauncher,
  createDaemonLaunch,
  registerSessionLaunchIpc,
  SHELL_CMD,
};
