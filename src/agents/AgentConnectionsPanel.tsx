import { useEffect, useRef, useState } from "react";
import { AgentConnectionCatalog } from "./AgentConnectionCatalog";
type Server = {
  id: string;
  name: string;
  transport: string;
  url?: string;
  command?: string;
  args: string[];
  environmentKeys: string[];
  auth: string;
  enabled: boolean;
  tools?: {
    include?: string[] | null;
    exclude?: string[];
    prompts?: boolean;
    resources?: boolean;
  };
};
type Flow = {
  status: string;
  authorizationUrl?: string | null;
  error?: string | null;
};
type Probe = {
  ok: boolean;
  error?: string;
  tools: { name: string; description: string }[];
  resources: number;
  prompts: number;
};
export function AgentConnectionsPanel({
  providerId,
  agentId,
  onClose,
}: {
  providerId: string;
  agentId: string;
  onClose: () => void;
}) {
  const [catalog, setCatalog] = useState(false);
  const [servers, setServers] = useState<Server[]>([]),
    [selected, setSelected] = useState<Server | null>(null);
  const [editing, setEditing] = useState<
    (Server & { revision: string }) | null
  >(null);
  const [creating, setCreating] = useState(false),
    [transport, setTransport] = useState("http"),
    [auth, setAuth] = useState("none");
  const [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [notice, setNotice] = useState("");
  const [flow, setFlow] = useState<Flow | null>(null),
    [probe, setProbe] = useState<Probe | null>(null),
    [removing, setRemoving] = useState(false);
  const version = useRef(0);
  const call = <T,>(action: string, input: Record<string, unknown> = {}) =>
    window.bridge!.agentCall<T>(providerId, `addons.mcp.${action}`, {
      agentId,
      ...input,
    });
  const run = async (work: () => Promise<void>) => {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await work();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const load = async () => {
    const r = await call<{ servers: Server[] }>("list");
    setServers(r.servers);
    setSelected((old) => r.servers.find((s) => s.id === old?.id) || null);
  };
  useEffect(() => {
    void run(load);
  }, [providerId, agentId]);
  useEffect(() => {
    const generation = ++version.current;
    setFlow(null);
    setProbe(null);
    setRemoving(false);
    if (!selected) return;
    const poll = async () => {
      try {
        const next = await call<Flow>("authStatus", { id: selected.id });
        if (generation !== version.current) return;
        setFlow(next);
      } catch (e) {
        if (generation === version.current) setError(String(e));
      }
    };
    void poll();
    return () => {
      version.current++;
    };
  }, [providerId, agentId, selected?.id]);
  useEffect(() => {
    if (
      !selected ||
      !["starting", "authorization_required"].includes(flow?.status || "")
    )
      return;
    const generation = version.current;
    let cancelled = false;
    const timer = setTimeout(
      () =>
        void call<Flow>("authStatus", { id: selected.id })
          .then((next) => {
            if (!cancelled && generation === version.current) setFlow(next);
          })
          .catch((e) => {
            if (!cancelled) setError(String(e));
          }),
      1500,
    );
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [flow, selected?.id]);
  if (catalog)
    return (
      <AgentConnectionCatalog
        providerId={providerId}
        agentId={agentId}
        onClose={() => {
          setCatalog(false);
          void run(load);
        }}
      />
    );
  return (
    <section className="agent-addon-panel">
      <header>
        <div>
          <h2>Connections</h2>
          <p className="agent-muted">
            Connect your agent to MCP tools and services.
          </p>
        </div>
        <button onClick={onClose}>Back to chat</button>
      </header>
      <div className="agent-actions">
        <button onClick={() => setCatalog(true)}>Browse catalog</button>
        <button
          disabled={busy}
          onClick={() => {
            setCreating(true);
            setEditing(null);
            setSelected(null);
            setError("");
          }}
        >
          Add connection
        </button>
        <button disabled={busy} onClick={() => void run(load)}>
          Refresh
        </button>
      </div>
      {error && (
        <p role="alert" className="agent-error">
          {error}
        </p>
      )}
      {notice && <p role="status">{notice}</p>}
      <div className="agent-resource-layout">
        <aside className="agent-connections-list">
          {servers.map((s) => (
            <button
              key={s.id}
              disabled={busy}
              className={selected?.id === s.id ? "selected" : ""}
              onClick={() => {
                setSelected(s);
                setEditing(null);
                setCreating(false);
              }}
            >
              <strong>{s.name}</strong>
              <small>
                {s.enabled ? "Enabled" : "Disabled"} · {s.transport}
              </small>
            </button>
          ))}
          {!servers.length && !busy && (
            <p className="agent-muted">No connections yet.</p>
          )}
        </aside>
        <div className="agent-resource-editor">
          {creating || editing ? (
            <form
              key={editing?.id || "new"}
              onSubmit={(e) => {
                e.preventDefault();
                const form = e.currentTarget,
                  data = new FormData(form);
                void run(async () => {
                  const input: Record<string, unknown> = {
                    id: editing?.id || data.get("name"),
                    ...(editing
                      ? { revision: editing.revision }
                      : { auth: transport === "http" ? auth : "none" }),
                  };
                  if (transport === "http") {
                    input.url = data.get("url");
                    if (!editing && auth === "header")
                      input.bearerToken = data.get("token");
                  } else {
                    input.command = data.get("command");
                    input.args = JSON.parse(String(data.get("args") || "[]"));
                    input.env = JSON.parse(String(data.get("env") || "{}"));
                    if (editing)
                      input.removeEnv = String(data.get("removeEnv") || "")
                        .split(",")
                        .map((s) => s.trim())
                        .filter(Boolean);
                  }
                  await call(editing ? "update" : "create", input);
                  form.reset();
                  setCreating(false);
                  setEditing(null);
                  await load();
                  setNotice(
                    "Connection saved. Changes apply to new agent sessions.",
                  );
                });
              }}
            >
              <label>
                Name
                <input
                  name="name"
                  defaultValue={editing?.id || ""}
                  disabled={!!editing}
                  aria-label="Connection name"
                  required
                  pattern="[a-zA-Z0-9][a-zA-Z0-9_.-]*"
                />
              </label>
              <label>
                Transport
                <select
                  value={transport}
                  disabled={!!editing}
                  onChange={(e) => setTransport(e.target.value)}
                >
                  <option value="http">Remote URL</option>
                  <option value="stdio">Local command</option>
                </select>
              </label>
              {transport === "http" ? (
                <>
                  <label>
                    Server URL
                    <input
                      name="url"
                      defaultValue={editing?.url || ""}
                      type="url"
                      placeholder="https://example.com/mcp"
                      required
                    />
                  </label>
                  <label>
                    Authentication
                    <select
                      value={auth}
                      disabled={!!editing}
                      onChange={(e) => setAuth(e.target.value)}
                    >
                      <option value="none">None</option>
                      <option value="oauth">Sign in with browser</option>
                      <option value="header">Bearer token</option>
                    </select>
                  </label>
                  {!editing && auth === "header" && (
                    <label>
                      Bearer token
                      <input
                        name="token"
                        type="password"
                        autoComplete="off"
                        required
                      />
                    </label>
                  )}
                </>
              ) : (
                <>
                  <label>
                    Command
                    <input
                      name="command"
                      defaultValue={editing?.command || ""}
                      placeholder="/path/to/server"
                      required
                    />
                  </label>
                  <label>
                    Arguments (JSON array)
                    <textarea
                      name="args"
                      defaultValue={JSON.stringify(editing?.args || [])}
                    />
                  </label>
                  <label>
                    {editing
                      ? "Add or replace environment (JSON object)"
                      : "Environment (JSON object)"}
                    <textarea
                      name="env"
                      defaultValue="{}"
                      autoComplete="off"
                      spellCheck={false}
                    />
                  </label>
                </>
              )}
              {editing && (
                <p className="agent-muted">
                  Saved credentials, authentication and tool access are
                  preserved. Only supplied environment values are replaced.
                </p>
              )}
              {editing && transport === "stdio" && (
                <label>
                  Remove environment variables (comma-separated)
                  <input
                    name="removeEnv"
                    placeholder={editing.environmentKeys.join(", ")}
                  />
                </label>
              )}
              <button disabled={busy} type="submit">
                Save connection
              </button>
              <button
                disabled={busy}
                type="button"
                onClick={() => {
                  setCreating(false);
                  setEditing(null);
                }}
              >
                Cancel
              </button>
            </form>
          ) : selected ? (
            <>
              <h3>{selected.name}</h3>
              <p className="agent-muted">
                {selected.url || [selected.command, ...selected.args].join(" ")}
              </p>
              {!!selected.environmentKeys.length && (
                <p>Environment: {selected.environmentKeys.join(", ")}</p>
              )}
              <div className="agent-actions">
                <button
                  disabled={busy}
                  onClick={() =>
                    void run(async () => {
                      const current = await call<Server & { revision: string }>(
                        "read",
                        { id: selected.id },
                      );
                      setTransport(current.transport);
                      setAuth(current.auth);
                      setEditing(current);
                    })
                  }
                >
                  Edit connection
                </button>
                <button
                  disabled={busy}
                  onClick={() =>
                    void run(async () => {
                      await call("toggle", {
                        id: selected.id,
                        enabled: !selected.enabled,
                      });
                      await load();
                      setNotice("Saved. Changes apply to new agent sessions.");
                    })
                  }
                >
                  {selected.enabled ? "Disable" : "Enable"}
                </button>
                <button
                  disabled={busy}
                  onClick={() =>
                    void run(async () =>
                      setProbe(await call<Probe>("test", { id: selected.id })),
                    )
                  }
                >
                  Test connection
                </button>
                {selected.auth === "oauth" && (
                  <button
                    disabled={busy}
                    onClick={() =>
                      void run(async () =>
                        setFlow(
                          await call<Flow>("authorize", { id: selected.id }),
                        ),
                      )
                    }
                  >
                    Sign in
                  </button>
                )}
                <button disabled={busy} onClick={() => setRemoving(true)}>
                  Remove
                </button>
              </div>
              {removing && (
                <div className="agent-delete-confirm">
                  <p>Remove this connection from this agent?</p>
                  <button
                    disabled={busy}
                    onClick={() =>
                      void run(async () => {
                        await call("remove", { id: selected.id });
                        setSelected(null);
                        await load();
                      })
                    }
                  >
                    Remove connection
                  </button>
                  <button onClick={() => setRemoving(false)}>
                    Keep connection
                  </button>
                </div>
              )}
              {flow && flow.status !== "none" && (
                <div role="status">
                  <strong>
                    {flow.status === "approved"
                      ? "Signed in"
                      : flow.status === "authorization_required"
                        ? "Waiting for sign-in"
                        : flow.status}
                  </strong>
                  {flow.error && <p>{flow.error}</p>}
                  {flow.authorizationUrl &&
                    flow.status === "authorization_required" && (
                      <button
                        onClick={() =>
                          void window
                            .bridge!.agentOpenExternal(flow.authorizationUrl!)
                            .catch((e) => setError(String(e)))
                        }
                      >
                        Continue in browser
                      </button>
                    )}
                  {["starting", "authorization_required"].includes(
                    flow.status,
                  ) && (
                    <button
                      onClick={() =>
                        void run(async () =>
                          setFlow(
                            await call<Flow>("cancelAuth", { id: selected.id }),
                          ),
                        )
                      }
                    >
                      Cancel sign-in
                    </button>
                  )}
                </div>
              )}
              <details>
                <summary>Tool access</summary>
                <form
                  key={`${selected.id}/${JSON.stringify(selected.tools)}`}
                  onSubmit={(e) => {
                    e.preventDefault();
                    const data = new FormData(e.currentTarget);
                    const lines = (name: string) =>
                      String(data.get(name) || "")
                        .split("\n")
                        .map((v) => v.trim())
                        .filter(Boolean);
                    void run(async () => {
                      await call("toolPolicy", {
                        id: selected.id,
                        include: data.has("onlyListed")
                          ? lines("include")
                          : null,
                        exclude: lines("exclude"),
                        prompts: data.has("prompts"),
                        resources: data.has("resources"),
                      });
                      await load();
                      setNotice(
                        "Tool access saved. Changes apply to new agent sessions.",
                      );
                    });
                  }}
                >
                  <label>
                    <input
                      type="checkbox"
                      name="onlyListed"
                      defaultChecked={Array.isArray(selected.tools?.include)}
                    />
                    Only allow listed tools
                  </label>
                  <label>
                    Allowed tools
                    <textarea
                      name="include"
                      defaultValue={(selected.tools?.include || []).join("\n")}
                      placeholder="One name or wildcard pattern per line"
                    />
                  </label>
                  <label>
                    Excluded tools
                    <textarea
                      name="exclude"
                      defaultValue={(selected.tools?.exclude || []).join("\n")}
                    />
                  </label>
                  <p className="agent-muted">
                    An enabled empty allow list blocks all tools. Allow lists
                    take precedence over exclusions.
                  </p>
                  <label>
                    <input
                      type="checkbox"
                      name="prompts"
                      defaultChecked={selected.tools?.prompts !== false}
                    />
                    Allow prompts
                  </label>
                  <label>
                    <input
                      type="checkbox"
                      name="resources"
                      defaultChecked={selected.tools?.resources !== false}
                    />
                    Allow resources
                  </label>
                  <button disabled={busy}>Save tool access</button>
                </form>
              </details>
              {probe && (
                <div>
                  <h4>
                    {probe.ok
                      ? `Connected · ${probe.tools.length} tools`
                      : "Connection failed"}
                  </h4>
                  {probe.error && <p role="alert">{probe.error}</p>}
                  {probe.ok && (
                    <p className="agent-muted">
                      {probe.resources} resources · {probe.prompts} prompts
                    </p>
                  )}
                  {probe.tools.map((tool) => (
                    <details key={tool.name}>
                      <summary>{tool.name}</summary>
                      <p>{tool.description}</p>
                    </details>
                  ))}
                </div>
              )}
            </>
          ) : (
            <p className="agent-muted">
              Select a connection to view its tools or manage access.
            </p>
          )}
        </div>
      </div>
    </section>
  );
}
