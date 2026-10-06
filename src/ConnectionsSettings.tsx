import { useCallback, useEffect, useState } from "react";
import {
  Check,
  Copy,
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
import type { ConnectionProfile, DaemonState } from "./types";
import {
  actionLabel,
  connectorFromLine,
  describeHost,
  formatArgv,
  type HostAction,
} from "./connectionState";

/** Daemon states by host, kept current by the bridge. */
function useDaemonStates(): Map<string, DaemonState> {
  const [states, setStates] = useState<Map<string, DaemonState>>(new Map());
  const accept = useCallback((next: DaemonState) => {
    setStates((current) => {
      const old = current.get(next.host);
      if (old && old.generation > next.generation) return current;
      return new Map(current).set(next.host, next);
    });
  }, []);
  useEffect(() => {
    let stopped = false;
    window.bridge
      ?.daemonStates()
      .then((all) => {
        if (!stopped) all.forEach(accept);
      })
      .catch(() => {});
    const off = window.bridge?.onDaemonState(accept);
    return () => {
      stopped = true;
      off?.();
    };
  }, [accept]);
  return states;
}

function CopyText({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <span className="copy-text">
      <code>{text}</code>
      <button
        type="button"
        title="Copy command"
        aria-label="Copy command"
        onClick={() => {
          void navigator.clipboard?.writeText(text);
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        }}
      >
        {copied ? <Check size={12} /> : <Copy size={12} />}
      </button>
    </span>
  );
}

type InstallState =
  | { phase: "running" }
  | { phase: "done"; version: string }
  | { phase: "error"; message: string };

function HostStatus({
  state,
  install,
  onAction,
}: {
  state: DaemonState | undefined;
  install?: InstallState;
  onAction(action: HostAction): void;
}) {
  const view = describeHost(state);
  const running = install?.phase === "running";
  return (
    <div className={`host-status tone-${view.tone}`}>
      <div className="host-status-line" role="status">
        <i className="status-dot" />
        <span>{view.label}</span>
        {view.detail && <small>{view.detail}</small>}
      </div>
      {view.hint && <p className="muted">{view.hint}</p>}
      {view.command && <CopyText text={view.command} />}
      {running && <p className="muted">Installing sushiai on the host…</p>}
      {install?.phase === "done" && (
        <p className="muted">Installed sushiai {install.version}.</p>
      )}
      {install?.phase === "error" && (
        <p className="inline-error" role="alert">
          {install.message}
        </p>
      )}
      {view.action && (
        <button
          type="button"
          className="secondary"
          disabled={running}
          onClick={() => onAction(view.action!)}
        >
          {actionLabel(view.action)}
        </button>
      )}
    </div>
  );
}

export function ConnectionsSettings({
  endpoint,
  localSocket,
  onSelect,
  profiles,
  onRefresh,
  notify,
}: {
  endpoint: string;
  localSocket: string;
  onSelect(endpoint: string): void;
  /** Shared with the Sidebar, which labels workspace groups by the same profiles. */
  profiles: ConnectionProfile[];
  onRefresh(): Promise<void>;
  notify(text: string): void;
}) {
  const states = useDaemonStates();
  const [installs, setInstalls] = useState<Record<string, InstallState>>({});
  async function install(id: string) {
    setInstalls((all) => ({ ...all, [id]: { phase: "running" } }));
    try {
      const result = await window.bridge!.hostInstall(id);
      setInstalls((all) => ({
        ...all,
        [id]: { phase: "done", version: result.version },
      }));
      await window.bridge!.connectionsConnect(`ssh:${id}`).catch(() => null);
    } catch (e) {
      setInstalls((all) => ({
        ...all,
        [id]: {
          phase: "error",
          message: e instanceof Error ? e.message : String(e),
        },
      }));
    }
  }
  const [editing, setEditing] = useState<ConnectionProfile | "new" | null>(
      null,
    ),
    [busy, setBusy] = useState(""),
    [error, setError] = useState("");
  async function connect(value: string, label: string) {
    setBusy(value);
    setError("");
    try {
      const { setup } = await window.bridge!.connectionsConnect(value);
      onSelect(value);
      notify(`Connecting to ${label}.` + (setup ? ` Set up: ${setup}.` : ""));
      await onRefresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  }
  return (
    <div className="connections-settings">
      <div className="connections-hosts">
        <div
          className={`connection-card ${!endpoint.startsWith("ssh:") ? "selected" : ""}`}
        >
          <Server size={17} />
          <button
            className="connection-info"
            onClick={() => onSelect(localSocket)}
          >
            <strong>This Mac</strong>
            <small>Local sushiai daemon</small>
          </button>
          {!endpoint.startsWith("ssh:") && <span>Active</span>}
          <HostStatus state={states.get("local")} onAction={() => {}} />
        </div>
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
                {p.port ? `:${p.port}` : ""}
                {p.connector?.kind === "command" ? " · custom command" : ""}
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
            <HostStatus
              state={states.get(p.id)}
              install={installs[p.id]}
              onAction={(action) =>
                action === "retry"
                  ? void connect(`ssh:${p.id}`, p.name)
                  : void install(p.id)
              }
            />
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
                  connector: connectorFromLine(String(form.get("command"))),
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
            <details
              className="advanced"
              open={
                typeof editing === "object" &&
                editing.connector?.kind === "command"
              }
            >
              <summary>Advanced</summary>
              <label>
                Connect with a command
                <input
                  name="command"
                  placeholder="Leave empty to use SSH"
                  defaultValue={
                    typeof editing === "object" &&
                    editing.connector?.kind === "command"
                      ? formatArgv(editing.connector.argv)
                      : ""
                  }
                />
              </label>
              <p className="muted">
                Runs the command and talks to the daemon over its input and
                output. Use quotes around arguments with spaces.
              </p>
            </details>
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
