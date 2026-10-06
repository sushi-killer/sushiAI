"use strict";

/**
 * Notices API: main-process, built-in extensions only. A source publishes a
 * notice; the app shows it as a native notification or in the mascot. The API
 * is generic and names no domain: the source maps its own data to a notice.
 *
 * @typedef {{
 *   id: string,
 *   label: string,
 *   emphasis?: "primary" | "ghost",
 *   icon?: "check" | "refresh",
 * }} NoticeAction
 * @typedef {{
 *   key: string,
 *   kind: "input" | "done" | "failed" | "info",
 *   title: string,
 *   body: string,
 *   label?: string,
 *   header?: string,
 *   at?: number,
 *   meta?: string[],
 *   choices?: string[],
 *   reply?: boolean,
 *   actions?: NoticeAction[],
 *   sticky?: boolean,
 * }} Notice
 * @typedef {(key: string, actionId: string, text?: string) => Promise<string | void>} NoticeActionHandler
 */

const KINDS = new Set(["input", "done", "failed", "info"]);
const ACTION_ID = /^[a-z][a-z0-9-]{0,39}$/;
const EMPHASES = new Set(["primary", "ghost"]);
const ICONS = new Set(["check", "refresh"]);
const MAX_SOURCE = 100;
const MAX_KEY = 1000;
const MAX_TITLE = 120;
const MAX_BODY = 2000;
const MAX_TEXT = 400;
const MAX_LIST = 8;
const MAX_ACTIONS = 4;
const NATIVE_BODY = 300;

function bounded(value, max) {
  return typeof value === "string" && value.length > 0 && value.length <= max;
}

function optionalText(value, max, name) {
  if (value === undefined) return undefined;
  if (!bounded(value, max)) throw new Error(`Invalid notice ${name}.`);
  return value;
}

function textList(value, name) {
  if (value === undefined) return undefined;
  if (
    !Array.isArray(value) ||
    value.length > MAX_LIST ||
    !value.every((item) => bounded(item, MAX_TEXT))
  )
    throw new Error(`Invalid notice ${name}.`);
  return [...value];
}

/** Checks a notice at the API boundary and returns a clean copy. Throws on
 * anything invalid; a source's data never reaches a window unchecked. */
function validateNotice(notice) {
  if (!notice || typeof notice !== "object" || Array.isArray(notice))
    throw new Error("Invalid notice.");
  if (!bounded(notice.key, MAX_KEY)) throw new Error("Invalid notice key.");
  if (!KINDS.has(notice.kind)) throw new Error("Invalid notice kind.");
  if (!bounded(notice.title, MAX_TITLE))
    throw new Error("Invalid notice title.");
  if (!bounded(notice.body, MAX_BODY)) throw new Error("Invalid notice body.");
  const actions = notice.actions ?? [];
  if (!Array.isArray(actions) || actions.length > MAX_ACTIONS)
    throw new Error("Invalid notice actions.");
  const clean = {
    key: notice.key,
    kind: notice.kind,
    title: notice.title,
    body: notice.body,
    actions: actions.map((action) => {
      if (
        !action ||
        typeof action !== "object" ||
        !ACTION_ID.test(action.id) ||
        !bounded(action.label, 40) ||
        (action.emphasis !== undefined && !EMPHASES.has(action.emphasis)) ||
        (action.icon !== undefined && !ICONS.has(action.icon))
      )
        throw new Error("Invalid notice action.");
      const item = { id: action.id, label: action.label };
      if (action.emphasis) item.emphasis = action.emphasis;
      if (action.icon) item.icon = action.icon;
      return item;
    }),
  };
  if (new Set(clean.actions.map((action) => action.id)).size !== actions.length)
    throw new Error("Duplicate notice action.");
  const label = optionalText(notice.label, 40, "label");
  if (label) clean.label = label;
  const header = optionalText(notice.header, MAX_TEXT, "header");
  if (header) clean.header = header;
  if (notice.at !== undefined) {
    if (!Number.isFinite(notice.at) || notice.at <= 0)
      throw new Error("Invalid notice time.");
    clean.at = notice.at;
  }
  const meta = textList(notice.meta, "meta");
  if (meta?.length) clean.meta = meta;
  const choices = textList(notice.choices, "choices");
  if (choices?.length) clean.choices = choices;
  if (notice.reply === true) clean.reply = true;
  if (notice.sticky === true) clean.sticky = true;
  return clean;
}

/**
 * @param {{
 *   preferences: { notifications: boolean, desktopMascot: boolean } | (() => { notifications: boolean, desktopMascot: boolean }),
 *   mascot: { add(notice: object): void, retract(source: string, key: string): void, clear(source?: string): void },
 *   showWindow: () => void,
 *   Notification: Function,
 *   icon?: unknown | (() => unknown),
 * }} deps
 */
function createNotices(deps) {
  const prefs = () =>
    typeof deps.preferences === "function"
      ? deps.preferences()
      : deps.preferences;
  /** @type {Map<string, NoticeActionHandler>} */
  const handlers = new Map();
  /** Native notifications that are still on screen, by source and key. */
  const natives = new Map();
  const nativeKey = (sourceId, key) => `${sourceId}\u0000${key}`;

  function closeNative(id) {
    const notification = natives.get(id);
    natives.delete(id);
    notification?.close();
  }

  function showNative(sourceId, notice) {
    const id = nativeKey(sourceId, notice.key);
    closeNative(id);
    const options = {
      title: notice.title,
      body: notice.body.slice(0, NATIVE_BODY),
    };
    const icon = typeof deps.icon === "function" ? deps.icon() : deps.icon;
    if (icon) options.icon = icon;
    const notification = new deps.Notification(options);
    natives.set(id, notification);
    const cleanup = () => {
      if (natives.get(id) === notification) natives.delete(id);
    };
    notification.on("click", () => {
      cleanup();
      deps.showWindow();
      Promise.resolve(handlers.get(sourceId)?.(notice.key, "open")).catch(
        () => {},
      );
    });
    notification.on("close", cleanup);
    notification.show();
  }

  return {
    /**
     * Register the handler that receives actions for a source. A click on a
     * native notification calls `onAction(key, "open")`. The handler may
     * return a message to show the user.
     * @param {string} sourceId
     * @param {NoticeActionHandler} onAction
     */
    register(sourceId, onAction) {
      if (!bounded(sourceId, MAX_SOURCE) || typeof onAction !== "function")
        throw new Error("Invalid notice source.");
      handlers.set(sourceId, onAction);
    },
    /**
     * Show or replace the notice `key` of a source. Dropped when notifications
     * are off; queued in the mascot when it is on; otherwise native.
     * @param {string} sourceId
     * @param {Notice} notice
     * @returns {"none" | "mascot" | "native"} where the notice went
     */
    publish(sourceId, notice) {
      if (!bounded(sourceId, MAX_SOURCE))
        throw new Error("Invalid notice source.");
      const clean = validateNotice(notice);
      const { notifications, desktopMascot } = prefs();
      if (!notifications) return "none";
      if (desktopMascot) {
        closeNative(nativeKey(sourceId, clean.key));
        deps.mascot.add({ source: sourceId, ...clean });
        return "mascot";
      }
      showNative(sourceId, clean);
      return "native";
    },
    /**
     * Run an action of a notice the user picked. The mascot calls this; a
     * click on a native notification calls it with the `open` action.
     * @param {string} sourceId
     * @param {string} key
     * @param {string} actionId
     * @param {string} [text]
     */
    async act(sourceId, key, actionId, text) {
      const handler = handlers.get(sourceId);
      if (!handler) throw new Error("That notice is gone.");
      return handler(key, actionId, text);
    },
    /**
     * Remove the notice `key` of a source, wherever it is shown.
     * @param {string} sourceId
     * @param {string} key
     */
    retract(sourceId, key) {
      closeNative(nativeKey(sourceId, key));
      deps.mascot.retract(sourceId, key);
    },
    /**
     * Remove every notice of a source, for example when it is disabled.
     * @param {string} sourceId
     */
    clear(sourceId) {
      const prefix = nativeKey(sourceId, "");
      for (const id of [...natives.keys()])
        if (id.startsWith(prefix)) closeNative(id);
      deps.mascot.clear(sourceId);
    },
  };
}

module.exports = { createNotices, validateNotice };
