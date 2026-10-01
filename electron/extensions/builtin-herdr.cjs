const path = require("node:path");
const { herdrLaunchParams } = require("../terminal-text.cjs");
const { request, inputCommands } = require("../herdr.cjs");
const { closeHerdrPane } = require("../herdr-pane-close.cjs");
const { HERDR_CONTRACT } = require("../herdr-contract.cjs");
const { checkHerdrCompatibility } = require("../herdr-compatibility.cjs");
const { HerdrEvents } = require("../herdr-events.cjs");
const { HerdrSnapshots } = require("../herdr-snapshots.cjs");
const {
  installPinnedHerdr,
  installRemoteHerdr,
} = require("../herdr-install.cjs");

const CLOSING = new Set(["pane.close", "workspace.close"]);
const GONE_MESSAGE = "That session is no longer open on the host.";

const HERDR_MANIFEST = {
  id: "builtin.herdr",
  name: "Herdr",
  version: "1.0.0",
  apiVersion: 1,
  source: { kind: "builtin" },
  scope: "app",
  description: "The bundled workspace session provider.",
  contributions: { surfaces: [], navigation: [], actions: [], commands: [] },
};

function registerHerdrExtension({
  handle,
  getConnections,
  id,
  send = () => {},
  executable = () => null,
}) {
  const inputQueues = new Map();
  const sessionQueues = new Map();
  function queueSession(socketPath, operation) {
    const next = (sessionQueues.get(socketPath) || Promise.resolve())
      .catch(() => {})
      .then(operation);
    sessionQueues.set(socketPath, next);
    return next.finally(() => {
      if (sessionQueues.get(socketPath) === next)
        sessionQueues.delete(socketPath);
    });
  }
  const snapshots = new HerdrSnapshots({
    getConnections,
    rpc: (socketPath, method) =>
      queueSession(socketPath, () => request(socketPath, method)),
  });
  const eventSubscriptions = new HerdrEvents({
    getConnections,
    send: (channel, event) => {
      snapshots.invalidate(event.endpoint);
      send(channel, event);
    },
  });
  const installs = new Map();
  const allowedMethods = new Set(
    HERDR_CONTRACT.requiredMethods.filter(
      (method) => method !== "events.subscribe",
    ),
  );

  handle("herdr", async (endpoint, method, params = {}) => {
    if (method === "session.snapshot") return snapshots.read(endpoint);
    const socketPath = await getConnections().socket(endpoint);
    if (
      typeof socketPath !== "string" ||
      !path.isAbsolute(socketPath) ||
      !allowedMethods.has(method)
    )
      throw new Error("Invalid Herdr request");
    if (method === "worktree.create") {
      const prepared = await getConnections().inspect(endpoint, {
        operation: "worktree_base",
        root: params.cwd,
        base: params.base,
      });
      params = { ...params, cwd: prepared.cwd, base: prepared.base };
    }
    if (method === "pane.send_input" && params.raw !== undefined) {
      if (typeof params.raw !== "string" || params.raw.length > 1000000)
        throw new Error("Input too large");
      const queueKey = socketPath + ":" + id(params.pane_id);
      const next = (inputQueues.get(queueKey) || Promise.resolve())
        .catch(() => {})
        .then(async () => {
          for (const command of inputCommands(params.raw))
            await request(socketPath, "pane.send_input", {
              pane_id: params.pane_id,
              ...command,
            });
        });
      inputQueues.set(queueKey, next);
      try {
        await next;
        return {};
      } finally {
        if (inputQueues.get(queueKey) === next) inputQueues.delete(queueKey);
      }
    }
    const changesState =
      method !== "ping" &&
      method !== "pane.read" &&
      method !== "pane.process_info";
    if (changesState) snapshots.invalidate(endpoint);
    try {
      if (method === "pane.close") {
        const paneId = id(params.pane_id);
        return await queueSession(socketPath, () =>
          closeHerdrPane(socketPath, paneId),
        );
      }
      if (method === "worktree.create")
        return await queueSession(socketPath, () =>
          request(socketPath, method, herdrLaunchParams(method, params)),
        );
      return await request(
        socketPath,
        method,
        herdrLaunchParams(method, params),
      );
    } catch (error) {
      if (!["pane_not_found", "workspace_not_found"].includes(error.code))
        throw error;
      if (CLOSING.has(method)) return { gone: true };
      error.message = GONE_MESSAGE;
      throw error;
    } finally {
      if (changesState) snapshots.invalidate(endpoint);
    }
  });
  handle("herdr-compatibility", (endpoint) =>
    checkHerdrCompatibility({
      endpoint,
      connections: getConnections(),
      binary: executable("herdr"),
    }),
  );
  handle("herdr-events-subscribe", (endpoint, subscriptionId) => {
    if (
      typeof endpoint !== "string" ||
      (!endpoint.startsWith("ssh:") && !path.isAbsolute(endpoint))
    )
      throw new Error("Invalid Herdr endpoint.");
    if (endpoint.startsWith("ssh:")) getConnections().get(endpoint);
    return eventSubscriptions.subscribe(endpoint, id(subscriptionId));
  });
  handle("herdr-events-unsubscribe", (endpoint, subscriptionId) =>
    eventSubscriptions.unsubscribe(endpoint, id(subscriptionId)),
  );
  handle("herdr-install", (endpoint) => {
    if (
      typeof endpoint !== "string" ||
      (!endpoint.startsWith("ssh:") && !path.isAbsolute(endpoint))
    )
      throw new Error("Invalid Herdr endpoint.");
    if (installs.has(endpoint)) return installs.get(endpoint);
    const connections = getConnections();
    const installation = Promise.resolve()
      .then(() =>
        endpoint.startsWith("ssh:")
          ? installRemoteHerdr(endpoint, connections)
          : installPinnedHerdr(connections.herdrInstallDirectory),
      )
      .finally(() => installs.delete(endpoint));
    installs.set(endpoint, installation);
    return installation;
  });
  return { snapshots, close: () => eventSubscriptions.close() };
}

module.exports = { HERDR_MANIFEST, registerHerdrExtension, GONE_MESSAGE };
