"use strict";

// IPC contract for the sushiai daemon. Handlers are stubs until lanes L1-L4b
// fill them in; argument validation already runs here and stays.

const { validatePanelId } = require("./panel-id.cjs");

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
  "host-install": (h) => host(h),
};

// `launch(request)` is the session launcher (electron/session-launch.cjs,
// createDaemonLaunch(...).launch); main.cjs passes it in.
function registerDaemonIpc({ handle, launch }) {
  for (const [channel, validate] of Object.entries(CHANNELS))
    handle(channel, async (...args) => {
      validate(...args);
      if (channel === "daemon-states") return [];
      if (channel === "daemon-session-launch" && launch) return launch(args[0]);
      throw new Error(NOT_IMPLEMENTED);
    });
}

module.exports = { registerDaemonIpc, CHANNELS, NOT_IMPLEMENTED };
