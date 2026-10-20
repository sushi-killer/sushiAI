"use strict";

// What each agent CLI of a host offers, from that host's own daemon
// (`agents.catalog`). The table lives in the Rust crate `sushiai-agents`;
// nothing here holds a model, an effort level or a permission mode.
//
// A host whose daemon is offline, older than this build or not running answers
// an empty catalog. A caller degrades with it - an empty picker - and never
// falls back to a list of its own, which is what this module replaces.

const METHOD = "agents.catalog";
const CAPABILITY = "agentsCatalog";
/** The daemon reads Codex's cache from disk; one read per minute is plenty. */
const TTL_MS = 60000;

/** `getManager()` is the daemon manager (null before it starts). `now` is
 * injectable so a test need not wait for the cache to expire. */
function createAgentCatalog({ getManager, now = () => Date.now() }) {
  const cache = new Map();

  /** Every agent of `host` ("local" or an ssh profile id), keyed by agent id. */
  async function of(host = "local") {
    const cached = cache.get(host);
    if (cached && now() - cached.at < TTL_MS) return cached.value;
    const value = await read(host);
    // Only a real answer is worth keeping: a daemon that is not up yet answers
    // nothing, and the next call must ask again rather than wait out the TTL.
    if (Object.keys(value).length) cache.set(host, { at: now(), value });
    return value;
  }

  async function read(host) {
    const manager = getManager?.();
    const state = manager?.states().find((item) => item.host === host);
    if (state?.state !== "ready") return {};
    // An older daemon does not serve the method: an empty catalog, not a guess.
    if (!state.capabilities?.includes(CAPABILITY)) return {};
    try {
      const list = await manager.request(host, METHOD, {});
      return Object.fromEntries((list || []).map((item) => [item.id, item]));
    } catch {
      return {};
    }
  }

  return { of, forget: (host) => cache.delete(host) };
}

module.exports = { createAgentCatalog };
