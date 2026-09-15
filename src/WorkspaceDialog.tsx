import { useEffect, useMemo, useState } from "react";
import { ArrowUpRight, FolderOpen } from "lucide-react";
import type { ConnectionProfile } from "./types";

/** Every host the new workspace could live on: this Mac, plus every
 * connected, non-hidden SSH profile. Defaults to whatever host the
 * currently active workspace is on, so creating one "just here" never has
 * to ask - but a dropdown still lets you point it somewhere else when more
 * than one host is actually live. */
function useHosts(
  localSocket: string,
  connectionProfiles: ConnectionProfile[],
  statusByEndpoint: Record<string, string>,
) {
  return useMemo(() => {
    const hosts: { endpoint: string; label: string; connected: boolean }[] = [];
    if (localSocket)
      hosts.push({
        endpoint: localSocket,
        label: "This Mac",
        connected: statusByEndpoint[localSocket] === "connected",
      });
    for (const profile of connectionProfiles) {
      if (!profile.connected || profile.hidden) continue;
      const endpoint = `ssh:${profile.id}`;
      hosts.push({
        endpoint,
        label: profile.name || profile.host,
        connected: statusByEndpoint[endpoint] === "connected",
      });
    }
    return hosts;
  }, [localSocket, connectionProfiles, statusByEndpoint]);
}

export function WorkspaceDialog({
  defaultCwd,
  activeEndpoint,
  localSocket,
  connectionProfiles,
  statusByEndpoint,
  onCreate,
}: {
  defaultCwd: string;
  activeEndpoint?: string;
  localSocket: string;
  connectionProfiles: ConnectionProfile[];
  statusByEndpoint: Record<string, string>;
  onCreate(
    name: string,
    cwd: string,
    backend: string,
    starter: string,
    endpoint?: string,
  ): Promise<void>;
}) {
  const hosts = useHosts(localSocket, connectionProfiles, statusByEndpoint);
  const [endpoint, setEndpoint] = useState(() => activeEndpoint || localSocket);
  const picked = hosts.find((h) => h.endpoint === endpoint);
  const host = {
    endpoint,
    remote: endpoint.startsWith("ssh:"),
    label: picked?.label ?? endpoint.replace(/^ssh:/, ""),
    connected: picked?.connected ?? false,
  };
  const connected = host.connected;
  const [cwd, setCwd] = useState(defaultCwd),
    [busy, setBusy] = useState(false),
    [backend, setBackend] = useState(
      host.remote || connected ? "herdr" : "local",
    );
  useEffect(() => {
    // A remote host is always Herdr-backed - picking one while "Local · built-in
    // PTY" was selected must not silently create a local workspace instead.
    if (host.remote) setBackend("herdr");
  }, [host.remote]);
  const remote = backend === "herdr" && host.remote;
  useEffect(() => {
    let cancelled = false;
    if (remote)
      window.bridge
        ?.projectInspect(host.endpoint, { operation: "home" })
        .then((info) => {
          if (!cancelled) setCwd(info.home);
        })
        .catch(() => {});
    else setCwd(defaultCwd);
    return () => {
      cancelled = true;
    };
  }, [remote, host.endpoint, defaultCwd]);
  return (
    <>
      <div className="dialog-eyebrow">A PLACE TO BUILD</div>
      <h2>
        New workspace{" "}
        <span className="remote-tag">
          <i className={`status-dot ${connected ? "green" : ""}`} />
          {host.label}
        </span>
      </h2>
      <p>Keep your agents and project together.</p>
      <form
        onSubmit={async (event) => {
          event.preventDefault();
          const data = new FormData(event.currentTarget);
          setBusy(true);
          try {
            await onCreate(
              String(data.get("name")),
              cwd,
              backend,
              String(data.get("starter")),
              backend === "herdr" ? host.endpoint : undefined,
            );
          } finally {
            setBusy(false);
          }
        }}
      >
        <label>
          Name
          <input
            name="name"
            autoFocus
            placeholder="my-next-project"
            required
            maxLength={80}
          />
        </label>
        <label>
          Project folder
          <div className="folder-field">
            <input
              value={cwd}
              onChange={(event) => setCwd(event.target.value)}
              required
              placeholder="/Users/you/projects/app"
            />
            <button
              type="button"
              title="Choose project folder"
              disabled={remote}
              onClick={async () => {
                const selected = await window.bridge?.chooseDirectory();
                if (selected) setCwd(selected);
              }}
            >
              <FolderOpen size={16} />
            </button>
          </div>
        </label>
        {hosts.length > 1 ? (
          <label>
            Host
            <select
              value={endpoint}
              onChange={(event) => setEndpoint(event.target.value)}
            >
              {hosts.map((h) => (
                <option key={h.endpoint} value={h.endpoint}>
                  {h.label}
                </option>
              ))}
            </select>
          </label>
        ) : null}
        {host.remote ? null : (
          <label>
            Session backend
            <select
              value={backend}
              onChange={(event) => setBackend(event.target.value)}
            >
              <option value="herdr" disabled={!connected}>
                Herdr · persistent sessions
              </option>
              <option value="local">Local · built-in PTY</option>
            </select>
            <small className="workspace-plugin-hint">
              {connected
                ? "Herdr sessions survive closing the app; local shells do not."
                : "Herdr is not connected, so this workspace runs on the built-in shell. Connect it in Settings to choose."}
            </small>
          </label>
        )}
        {host.remote && !connected ? (
          <small className="workspace-plugin-hint">
            {host.label} is offline right now - reconnect it in Settings before
            creating a workspace there.
          </small>
        ) : null}
        <label>
          Agent / harness
          <select name="starter">
            <option value="shell">One terminal</option>
            <option value="claude">Claude Code</option>
            <option value="codex">Codex</option>
            <option value="gemini">Gemini CLI</option>
          </select>
        </label>
        <button
          type="submit"
          className="primary"
          disabled={busy || (host.remote && !connected)}
        >
          {busy ? "Creating…" : "Create workspace"}
          <ArrowUpRight size={14} />
        </button>
      </form>
    </>
  );
}
