"use strict";

// The owner cards for companion host.exec calls. Main owns the queue: one card
// is on screen at a time across all companions, the 2 minutes start when a
// card is shown, and an Allow earlier than MIN_SHOWN_MS after that is ignored
// (a click that was meant for something under the card). The renderer only
// draws what main announces and withdraws what main withdraws.

const { randomUUID } = require("node:crypto");

const ASK_TIMEOUT_MS = 2 * 60 * 1000;
const MIN_SHOWN_MS = 1000;

/** `announce(card)` shows a card; `withdraw({id})` removes it. */
function createOwnerPrompts({
  announce,
  withdraw,
  now = Date.now,
  timeoutMs = ASK_TIMEOUT_MS,
  minShownMs = MIN_SHOWN_MS,
}) {
  const queue = [];
  let active = null;

  function settle(entry, decision) {
    if (entry.done) return;
    entry.done = true;
    clearTimeout(entry.timer);
    if (active === entry) {
      active = null;
      withdraw({ id: entry.id });
    } else {
      const index = queue.indexOf(entry);
      if (index >= 0) queue.splice(index, 1);
    }
    entry.resolve(decision);
    next();
  }

  function next() {
    if (active || !queue.length) return;
    active = queue.shift();
    active.shownAt = now();
    active.timer = setTimeout(settle, timeoutMs, active, "timeout");
    active.timer.unref?.();
    announce({ id: active.id, ...active.question });
  }

  return {
    /** Resolves "allowed", "denied" or "timeout". */
    ask(question) {
      return new Promise((resolve) => {
        queue.push({ id: randomUUID(), question, resolve, done: false });
        next();
      });
    },
    /** The renderer's answer. Only the id on screen is accepted, once. */
    answer(id, allow) {
      if (!active || active.id !== id) return;
      if (allow === true && now() - active.shownAt < minShownMs) return;
      settle(active, allow === true ? "allowed" : "denied");
    },
    /** A companion restarted: its cards, shown or waiting, are void. */
    cancel(extensionId) {
      for (const entry of [active, ...queue])
        if (entry?.question.extensionId === extensionId)
          settle(entry, "denied");
    },
  };
}

module.exports = { createOwnerPrompts, ASK_TIMEOUT_MS, MIN_SHOWN_MS };
