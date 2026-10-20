"use strict";

// What each agent CLI of a host offers, from that host's own daemon
// (`agents.catalog`). A daemon that is offline or older than this build answers
// an empty catalog; callers degrade with it and keep no list of their own.

const METHOD = "agents.catalog";
const CAPABILITY = "agentsCatalog";

/** `getManager()` is the daemon manager (null before it starts). */
function createAgentCatalog({ getManager }) {
  /** Every agent of `host` ("local" or an ssh profile id), keyed by agent id. */
  async function of(host = "local") {
    const manager = getManager?.();
    const state = manager?.states().find((item) => item.host === host);
    if (state?.state !== "ready") return {};
    if (!state.capabilities?.includes(CAPABILITY)) return {};
    try {
      const list = await manager.request(host, METHOD, {});
      return Object.fromEntries((list || []).map((item) => [item.id, item]));
    } catch {
      return {};
    }
  }

  return { of };
}

module.exports = { createAgentCatalog };
