"use strict";

// Connect on a host the daemon manager owns. The auto-connect flag is set
// before the (slow) retry is awaited: a Disconnect clicked meanwhile clears it
// afterwards, and the late connect result must not turn it back on.
const connectThroughDaemon =
  ({ daemonHost, getManager, getConnections }) =>
  (original) =>
  async (endpoint, ...rest) => {
    const host = daemonHost(endpoint);
    if (!host) return original(endpoint, ...rest);
    const manager = getManager();
    await getConnections().setAutoConnect(endpoint, true);
    let state = manager.states().find((item) => item.host === host);
    if (state.state !== "ready" && state.state !== "connecting")
      state = await manager.retry(host);
    return { connected: state.state === "ready", setup: "" };
  };

module.exports = { connectThroughDaemon };
