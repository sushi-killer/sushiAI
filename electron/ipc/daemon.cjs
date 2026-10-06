"use strict";

// IPC contract for the sushiai daemon. Lifecycle handlers dispatch to the
// daemon manager; launch, terminal and host-install stay stubs until their
// lanes fill them in. Argument validation runs here before any dispatch.

const { validatePanelId } = require("./panel-id.cjs");
const { createTerminalHandlers } = require("../daemon/terminals.cjs");

const NOT_IMPLEMENTED = "daemon IPC not implemented yet";
const MAX_TEXT = 4096;

function text(value, name, { optional = false, max = MAX_TEXT } = {}) {
  if (optional && value === undefined) return value;
  if (typeof value !== "string" || !value || value.length > max)
    throw new Error(`Invalid ${name}.`);
  return value;
}
function int(value, name, { optional = false, min = 0, max = 1e9 } = {}) {
  if (optional && value === undefined) return value;
  if (!Number.isInteger(value) || value < min || value > max)
    throw new Error(`Invalid ${name}.`);
  return value;
}
function object(value, name) {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error(`Invalid ${name}.`);
  return value;
}
function strings(value, name) {
  if (value === undefined) return value;
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string"))
    throw new Error(`Invalid ${name}.`);
  return value;
}
const host = (value) => text(value, "host", { max: 256 });
const size = (cols, rows) => {
  int(cols, "cols", { min: 1, max: 1000 });
  int(rows, "rows", { min: 1, max: 1000 });
};

// channel -> argument validator. A validator throws on bad arguments.
const CHANNELS = {
  "daemon-states": () => {},
  "daemon-sessions-list": (h) => host(h),
  "daemon-session-launch": (request) => {
    const r = object(request, "launch request");
    host(r.host);
    text(r.cwd, "cwd");
    text(r.idempotencyKey, "idempotencyKey", { max: 256 });
    size(r.cols, r.rows);
    for (const key of [
      "agent",
      "title",
      "model",
      "prompt",
      "resume",
      "project",
      "group",
      "claudeAccountId",
      "codexAccountId",
      "modelProfileId",
    ])
      if (r[key] !== undefined && typeof r[key] !== "string")
        throw new Error(`Invalid ${key}.`);
    strings(r.extraArgs, "extraArgs");
  },
  "daemon-session-close": (h, id, graceful) => {
    host(h);
    text(id, "session id");
    if (typeof graceful !== "boolean") throw new Error("Invalid graceful.");
  },
  "daemon-session-remove": (h, id) => {
    host(h);
    text(id, "session id");
  },
  "daemon-session-update": (h, patch) => {
    host(h);
    text(object(patch, "update").id, "session id");
  },
  "daemon-session-read": (h, id, scrollback) => {
    host(h);
    text(id, "session id");
    int(scrollback, "scrollback", { optional: true, max: 1e6 });
  },
  "daemon-session-input": (h, id, data) => {
    host(h);
    text(id, "session id");
    if (typeof data !== "string") throw new Error("Invalid data.");
  },
  "daemon-ask-respond": (h, response) => {
    host(h);
    const r = object(response, "ask response");
    text(r.sessionId, "session id");
    text(r.askId, "ask id");
    if (r.decision !== "allow" && r.decision !== "deny")
      throw new Error("Invalid decision.");
  },
  "daemon-terminal-attach": (input) => {
    const r = object(input, "attach request");
    validatePanelId(r.panelId);
    host(r.host);
    text(r.sessionId, "session id");
    size(r.cols, r.rows);
    int(r.scrollback, "scrollback", { optional: true, max: 1e6 });
  },
  "daemon-terminal-write": (panelId, data) => {
    validatePanelId(panelId);
    if (typeof data !== "string") throw new Error("Invalid data.");
  },
  "daemon-terminal-resize": (panelId, cols, rows) => {
    validatePanelId(panelId);
    size(cols, rows);
  },
  "daemon-terminal-detach": (panelId) => validatePanelId(panelId),
  "daemon-terminal-ack": (panelId, bytes) => {
    validatePanelId(panelId);
    int(bytes, "bytes");
  },
  "daemon-terminal-attach-file": (panelId, path) => {
    validatePanelId(panelId);
    text(path, "path");
  },
  "daemon-terminal-attach-data": (panelId, name, bytes) => {
    validatePanelId(panelId);
    text(name, "name", { max: 256 });
    if (!(bytes instanceof Uint8Array)) throw new Error("Invalid data.");
  },
  "host-install": (h) => host(h),
};

// Lifecycle channels -> [daemon method, params from the validated arguments].
const LIFECYCLE = {
  "daemon-sessions-list": () => ["session.list", {}],
  "daemon-session-close": (_h, id, graceful) => [
    "session.close",
    { id, graceful },
  ],
  "daemon-session-remove": (_h, id) => ["session.remove", { id }],
  "daemon-session-update": (_h, patch) => {
    const { id, project, group, title } = patch;
    return ["session.update", { id, project, group, title }];
  },
  "daemon-session-read": (_h, id, scrollback) => [
    "session.read",
    { id, scrollback },
  ],
  "daemon-session-input": (_h, id, data) => ["session.input", { id, data }],
  "daemon-ask-respond": (_h, { askId, decision, message }) => [
    "ask.respond",
    { askId, decision, message },
  ],
};
// Channels whose result the renderer reads; the others resolve to undefined.
const RETURNS_RESULT = new Set(["daemon-sessions-list", "daemon-session-read"]);

// daemon-terminal-* channel -> method of the terminal handlers.
const TERMINAL_CHANNELS = {
  "daemon-terminal-attach": (t, input) => t.attach(input),
  "daemon-terminal-write": (t, panelId, data) => t.write(panelId, data),
  "daemon-terminal-resize": (t, panelId, cols, rows) =>
    t.resize(panelId, cols, rows),
  "daemon-terminal-detach": (t, panelId) => t.detach(panelId),
  "daemon-terminal-ack": (t, panelId, bytes) => t.ack(panelId, bytes),
  "daemon-terminal-attach-file": (t, panelId, path) =>
    t.attachFile(panelId, path),
  "daemon-terminal-attach-data": (t, panelId, name, bytes) =>
    t.attachData(panelId, name, bytes),
};

// `launch(request)` is the session launcher (electron/session-launch.cjs,
// createDaemonLaunch(...).launch); main.cjs passes it in.
function registerDaemonIpc({
  handle,
  launch,
  send = () => {},
  getManager = () => null,
  onEvent,
  attachmentsDir,
}) {
  const terminals = createTerminalHandlers({
    getManager,
    send,
    onEvent,
    attachmentsDir,
  });
  for (const [channel, validate] of Object.entries(CHANNELS))
    handle(channel, async (...args) => {
      validate(...args);
      const manager = getManager();
      if (channel === "daemon-states") return manager ? manager.states() : [];
      if (channel === "daemon-session-launch" && launch) return launch(args[0]);
      const build = LIFECYCLE[channel];
      const terminal = TERMINAL_CHANNELS[channel];
      if (!build && !terminal) throw new Error(NOT_IMPLEMENTED);
      if (!manager) throw new Error("daemon manager is not running");
      if (terminal) return terminal(terminals, ...args);
      const [method, params] = build(...args);
      const result = await manager.request(args[0], method, params);
      return RETURNS_RESULT.has(channel) ? result : undefined;
    });
  return { getManager, terminals };
}

/** Pushes manager state and events to every window; returns the unsubscribe. */
function forwardDaemonEvents(manager, broadcast) {
  const offs = [
    manager.on("state", (state) => broadcast("daemon-state", state)),
    manager.on("event", (event) => broadcast("daemon-event", event)),
  ];
  return () => offs.forEach((off) => off());
}

module.exports = {
  registerDaemonIpc,
  forwardDaemonEvents,
  CHANNELS,
  NOT_IMPLEMENTED,
};
