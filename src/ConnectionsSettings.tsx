import { useEffect, useState } from "react";
import { Globe, Link, Plus, Server, Trash2, Unplug } from "lucide-react";
import type { ConnectionProfile } from "./types";
export function ConnectionsSettings({
  endpoint,
  localSocket,
  onSelect,
}: {
  endpoint: string;
  localSocket: string;
  onSelect(endpoint: string): void;
}) {
  const [profiles, setProfiles] = useState<ConnectionProfile[]>([]),
    [editing, setEditing] = useState(false),
    [busy, setBusy] = useState(""),
    [error, setError] = useState("");
  const refresh = () =>
    window.bridge
      ?.connectionsList()
      .then(setProfiles)
      .catch((e) => setError(e.message));
  useEffect(() => {
    refresh();
  }, []);
  async function connect(value: string) {
    setBusy(value);
    setError("");
    try {
      await window.bridge!.connectionsConnect(value);
      onSelect(value);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  }
  return (
    <div className="connections-settings">
      <div className="settings-divider" />
      <div className="setting-heading">
        <h3>Connections</h3>
        <button onClick={() => setEditing(!editing)}>
          <Plus size={13} /> Add SSH host
        </button>
      </div>
      <button
        className={`connection-card ${!endpoint.startsWith("ssh:") ? "selected" : ""}`}
        onClick={() => onSelect(localSocket)}
      >
        <Server size={17} />
        <div>
          <strong>This Mac</strong>
          <small>Local Herdr</small>
        </div>
        {!endpoint.startsWith("ssh:") && <span>Active</span>}
      </button>
      {profiles.map((p) => (
        <div
          className={`connection-card ${endpoint === `ssh:${p.id}` ? "selected" : ""}`}
          key={p.id}
        >
          <Globe size={17} />
          <button
            className="connection-info"
            onClick={() => connect(`ssh:${p.id}`)}
          >
            <strong>{p.name}</strong>
            <small>
              {p.host}
              {p.port ? `:${p.port}` : ""} · {p.socket}
            </small>
          </button>
          <button
            title={`Connect ${p.name}`}
            disabled={!!busy}
            onClick={() => connect(`ssh:${p.id}`)}
          >
            <Link size={14} />
          </button>
          <button
            title={`Disconnect ${p.name}`}
            onClick={async () => {
              await window.bridge!.connectionsDisconnect(`ssh:${p.id}`);
              if (endpoint === `ssh:${p.id}`) onSelect(localSocket);
              refresh();
            }}
          >
            <Unplug size={14} />
          </button>
          <button
            title={`Remove connection ${p.name}`}
            onClick={async () => {
              await window.bridge!.connectionsDelete(`ssh:${p.id}`);
              if (endpoint === `ssh:${p.id}`) onSelect(localSocket);
              refresh();
            }}
          >
            <Trash2 size={13} />
          </button>
        </div>
      ))}
      {editing && (
        <form
          className="ssh-form"
          onSubmit={async (event) => {
            event.preventDefault();
            const form = new FormData(event.currentTarget);
            setError("");
            try {
              const p = await window.bridge!.connectionsSave({
                name: String(form.get("name")),
                host: String(form.get("host")),
                port: Number(form.get("port")) || undefined,
                socket: String(form.get("socket")),
              });
              setEditing(false);
              await refresh();
              await connect(`ssh:${p.id}`);
            } catch (e) {
              setError(String(e));
            }
          }}
        >
          <label>
            Connection name
            <input name="name" placeholder="Lab" required />
          </label>
          <div className="form-row">
            <label>
              SSH alias or user@host
              <input name="host" placeholder="lab" required />
            </label>
            <label>
              Port
              <input
                name="port"
                type="number"
                min="1"
                max="65535"
                placeholder="From SSH config"
              />
            </label>
          </div>
          <label>
            Remote Herdr socket
            <input
              name="socket"
              defaultValue="~/.config/herdr/herdr.sock"
              required
            />
          </label>
          <p className="muted">
            Uses your SSH config, keys and agent. Connect once in Terminal to
            verify a new host’s key. Remote file browsing requires Python 3.
          </p>
          <button className="primary" disabled={!!busy} type="submit">
            Save and connect
          </button>
        </form>
      )}
      {busy && <p className="muted">Connecting over SSH…</p>}
      {error && (
        <p className="inline-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
