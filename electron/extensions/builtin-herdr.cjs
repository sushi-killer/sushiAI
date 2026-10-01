const path = require("node:path");
const { herdrLaunchParams } = require("../terminal-text.cjs");
const { request, inputCommands } = require("../herdr.cjs");

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

/** Herdr says a pane or workspace is not there (closed, Herdr restarted, a
 * stale id kept in app state). */
const GONE = /\b(?:pane|workspace|worktree|tab)\b[^\n]*\bnot found\b/i;
const CLOSING = new Set(["pane.close", "workspace.close"]);
const GONE_MESSAGE = "That session is no longer open on the host.";

function registerHerdrExtension({ handle, getConnections, id }) {
  const inputQueues = new Map();
  const allowedMethods = new Set([
    "ping",
    "session.snapshot",
    "workspace.create",
    "workspace.rename",
    "worktree.create",
    "pane.rename",
    "pane.split",
    "pane.read",
    "pane.send_text",
    "pane.send_keys",
    "pane.send_input",
    "pane.close",
    "workspace.close",
  ]);

  handle("herdr", async (endpoint, method, params = {}) => {
    const socketPath = await getConnections().socket(endpoint);
    if (
      typeof socketPath !== "string" ||
      !path.isAbsolute(socketPath) ||
      !allowedMethods.has(method)
    )
      throw new Error("Invalid Herdr request");
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
    try {
      return await request(
        socketPath,
        method,
        herdrLaunchParams(method, params),
      );
    } catch (error) {
      if (!GONE.test(error?.message ?? "")) throw error;
      // Closing what is already closed is closed: not an error.
      if (CLOSING.has(method)) return { gone: true };
      throw Object.assign(new Error(GONE_MESSAGE), { code: "HERDR_GONE" });
    }
  });
}

module.exports = { HERDR_MANIFEST, registerHerdrExtension, GONE_MESSAGE };
