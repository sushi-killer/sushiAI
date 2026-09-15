import { useState, type ReactNode } from "react";
import {
  Eye,
  EyeOff,
  Globe,
  Link,
  Pencil,
  Plus,
  Server,
  Trash2,
  Unplug,
} from "lucide-react";
import type { ConnectionProfile } from "./types";
export function ConnectionsSettings({
  endpoint,
  localSocket,
  onSelect,
  socketForm,
  profiles,
  onRefresh,
  notify,
}: {
  endpoint: string;
  localSocket: string;
  onSelect(endpoint: string): void;
  /** The Herdr socket form, owned by App because it drives the connection. */
  socketForm?: ReactNode;
  /** Shared with the Sidebar, which labels workspace groups by the same profiles. */
  profiles: ConnectionProfile[];
  onRefresh(): Promise<void>;
  notify(text: string): void;
}) {
  const [editing, setEditing] = useState<ConnectionProfile | "new" | null>(
      null,
    ),
    [busy, setBusy] = useState(""),
    [error, setError] = useState("");
  async function connect(value: string, label: string) {
    setBusy(value);
    setError("");
    try {
      await window.bridge!.connectionsConnect(value);
      onSelect(value);
      const response = await window
        .bridge!.herdr(value, "session.snapshot")
        .catch(() => null);
      const count = (response?.snapshot ?? response)?.workspaces?.length;
      notify(
        typeof count === "number"
          ? `Connected to ${label}: ${count} workspace${count === 1 ? "" : "s"}.`
          : `Connected to ${label}.`,
      );
      await onRefresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  }
  return (
    <div className="connections-settings">
      {socketForm}
      <div className="connections-hosts">
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
            className={`connection-card ${endpoint === `ssh:${p.id}` ? "selected" : ""} ${p.hidden ? "hidden-from-sidebar" : ""}`}
            key={p.id}
          >
            <Globe size={17} />
            <button
              className="connection-info"
              onClick={() => connect(`ssh:${p.id}`, p.name)}
            >
              <strong>{p.name}</strong>
              <small>
                {p.host}
                {p.port ? `:${p.port}` : ""} · {p.socket}
                {p.hidden ? " · hidden from Workspaces" : ""}
              </small>
            </button>
            <button
              title={`Connect ${p.name}`}
              disabled={!!busy}
              onClick={() => connect(`ssh:${p.id}`, p.name)}
            >
              <Link size={14} />
            </button>
            <button title={`Edit ${p.name}`} onClick={() => setEditing(p)}>
              <Pencil size={13} />
            </button>
            <button
              title={
                p.hidden
                  ? `Show ${p.name} in Workspaces`
                  : `Hide ${p.name} from Workspaces`
              }
              onClick={async () => {
                await window.bridge!.connectionsSetHidden(
                  `ssh:${p.id}`,
                  !p.hidden,
                );
                await onRefresh();
              }}
            >
              {p.hidden ? <EyeOff size={14} /> : <Eye size={14} />}
            </button>
            <button
              title={`Disconnect ${p.name}`}
              onClick={async () => {
                await window.bridge!.connectionsDisconnect(`ssh:${p.id}`);
                if (endpoint === `ssh:${p.id}`) onSelect(localSocket);
                await onRefresh();
              }}
            >
              <Unplug size={14} />
            </button>
            <button
              title={`Remove connection ${p.name}`}
              onClick={async () => {
                await window.bridge!.connectionsDelete(`ssh:${p.id}`);
                if (endpoint === `ssh:${p.id}`) onSelect(localSocket);
                await onRefresh();
              }}
            >
              <Trash2 size={13} />
            </button>
          </div>
        ))}
        {editing ? (
          <form
            className="ssh-form"
            onSubmit={async (event) => {
              event.preventDefault();
              const form = new FormData(event.currentTarget);
              setError("");
              try {
                const p = await window.bridge!.connectionsSave({
                  id: typeof editing === "object" ? editing.id : undefined,
                  name: String(form.get("name")),
                  host: String(form.get("host")),
                  port: Number(form.get("port")) || undefined,
                  socket: String(form.get("socket")),
                });
                setEditing(null);
                await onRefresh();
                await connect(`ssh:${p.id}`, p.name);
              } catch (e) {
                setError(String(e));
              }
            }}
          >
            <label>
              Connection name
              <input
                name="name"
                placeholder="Lab"
                defaultValue={
                  typeof editing === "object" ? editing.name : undefined
                }
                required
              />
            </label>
            <div className="form-row">
              <label>
                SSH alias or user@host
                <input
                  name="host"
                  placeholder="lab"
                  defaultValue={
                    typeof editing === "object" ? editing.host : undefined
                  }
                  required
                />
              </label>
              <label>
                Port
                <input
                  name="port"
                  type="number"
                  min="1"
                  max="65535"
                  placeholder="From SSH config"
                  defaultValue={
                    typeof editing === "object" ? editing.port : undefined
                  }
                />
              </label>
            </div>
            <label>
              Remote Herdr socket
              <input
                name="socket"
                defaultValue={
                  typeof editing === "object"
                    ? editing.socket
                    : "~/.config/herdr/herdr.sock"
                }
                required
              />
            </label>
            <p className="muted">
              Uses your SSH config, keys and agent. Connect once in Terminal to
              verify a new host’s key. Remote file browsing requires Python 3.
              To connect as a different Unix user, use <code>user@host</code>{" "}
              (e.g. <code>user@devbox</code>).
            </p>
            <div className="dialog-actions">
              <button
                className="secondary"
                type="button"
                onClick={() => setEditing(null)}
              >
                Cancel
              </button>
              <button className="primary" disabled={!!busy} type="submit">
                Save and connect
              </button>
            </div>
          </form>
        ) : (
          <button className="add-row" onClick={() => setEditing("new")}>
            <Plus size={12} /> Add SSH host
          </button>
        )}
        {busy && <p className="muted">Connecting over SSH…</p>}
        {error && (
          <p className="inline-error" role="alert">
            {error}
          </p>
        )}
      </div>
    </div>
  );
}
