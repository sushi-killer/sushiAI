function registerProjectIpc({
  handle,
  getConnections,
  getPreview,
  terminals,
  terminalPending,
}) {
  const connections = () => {
    const value = getConnections();
    if (!value) throw new Error("Connections are not ready.");
    return value;
  };

  async function disconnectEndpoint(endpoint) {
    for (const pending of terminalPending.values()) {
      if (pending.endpoint === endpoint) pending.cancelled = true;
    }
    for (const terminal of terminals.values()) {
      if (terminal.endpoint === endpoint && !terminal.exited)
        terminal.proc.kill();
    }
    await connections().disconnect(endpoint);
    if (endpoint?.startsWith("ssh:"))
      await connections().setAutoConnect(endpoint, false);
  }

  handle("connections-list", () => connections().list());
  handle("connections-save", (profile) => connections().save(profile));
  handle("connections-set-hidden", (endpoint, hidden) =>
    connections().setHidden(endpoint, hidden),
  );
  handle("connections-delete", async (endpoint) => {
    await disconnectEndpoint(endpoint);
    await connections().delete(endpoint);
  });
  handle("connections-connect", async (endpoint) => {
    await connections().socket(endpoint);
    if (endpoint?.startsWith("ssh:"))
      await connections().setAutoConnect(endpoint, true);
    return { connected: true };
  });
  handle("connections-disconnect", disconnectEndpoint);
  handle("connections-forward", async (endpoint, url) => {
    const parsed = new URL(url);
    if (
      !["http:", "https:"].includes(parsed.protocol) ||
      !["localhost", "127.0.0.1", "[::1]"].includes(parsed.hostname)
    )
      throw new Error("Forwarding supports only a remote localhost URL.");
    const local = await connections().forward(
      endpoint,
      Number(parsed.port) || (parsed.protocol === "https:" ? 443 : 80),
    );
    parsed.hostname = "127.0.0.1";
    parsed.port = String(local);
    return parsed.href;
  });

  handle("project-inspect", (endpoint, options) => {
    if (
      ![
        "list",
        "read",
        "write",
        "git",
        "diff",
        "home",
        "log",
        "commit",
        "branches",
        "git_overview",
        "git_remote",
        "checkout",
      ].includes(options?.operation)
    )
      throw new Error("Unknown project operation");
    return connections().inspect(endpoint, options);
  });

  handle("project-preview", async (endpoint, root, file) => {
    await connections().inspect(endpoint, {
      operation: "read",
      root,
      path: file,
    });
    return getPreview().grant(endpoint, root, file);
  });
}

module.exports = { registerProjectIpc };
