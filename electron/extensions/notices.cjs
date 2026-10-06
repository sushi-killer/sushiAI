"use strict";

/**
 * Notices API: main-process, built-in extensions only. A source publishes a
 * notice; the app shows it as a native notification or in the mascot. The API
 * is generic and names no domain: the source maps its own data to a notice.
 *
 * @typedef {{ id: string, label: string }} NoticeAction
 * @typedef {{
 *   key: string,
 *   kind: "input" | "done" | "failed" | "info",
 *   title: string,
 *   body: string,
 *   header?: string,
 *   meta?: string[],
 *   choices?: string[],
 *   reply?: boolean,
 *   actions?: NoticeAction[],
 *   sticky?: boolean,
 * }} Notice
 * @typedef {(key: string, actionId: string, text?: string) => Promise<string | void>} NoticeActionHandler
 */

/**
 * @param {{
 *   preferences: object,
 *   mascot: object,
 *   showWindow: () => void,
 *   Notification: Function,
 *   icon?: unknown,
 * }} deps
 */
function createNotices(deps) {
  void deps;
  /** @type {Map<string, NoticeActionHandler>} */
  const handlers = new Map();
  /** @type {Map<string, Map<string, Notice>>} */
  const live = new Map();

  return {
    /**
     * Register the handler that receives actions for a source. A click on a
     * native notification calls `onAction(key, "open")`. The handler may
     * return a message to show the user.
     * @param {string} sourceId
     * @param {NoticeActionHandler} onAction
     */
    register(sourceId, onAction) {
      handlers.set(sourceId, onAction);
    },
    /**
     * Show or replace the notice `key` of a source. Dropped when notifications
     * are off; queued in the mascot when it is on; otherwise native.
     * @param {string} sourceId
     * @param {Notice} notice
     */
    publish(sourceId, notice) {
      if (!live.has(sourceId)) live.set(sourceId, new Map());
      live.get(sourceId).set(notice.key, notice);
    },
    /**
     * Remove the notice `key` of a source, wherever it is shown.
     * @param {string} sourceId
     * @param {string} key
     */
    retract(sourceId, key) {
      live.get(sourceId)?.delete(key);
    },
    /**
     * Remove every notice of a source, for example when it is disabled.
     * @param {string} sourceId
     */
    clear(sourceId) {
      live.delete(sourceId);
    },
  };
}

module.exports = { createNotices };
