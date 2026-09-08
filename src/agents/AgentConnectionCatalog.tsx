import { useEffect, useRef, useState } from "react";
type Entry = {
  name: string;
  description: string;
  transport: string;
  authType: string;
  requiredEnv: { name: string; prompt: string; required: boolean }[];
  command?: string;
  args: string[];
  url?: string;
  installUrl?: string;
  installRef?: string;
  bootstrap: string[];
  postInstall: string;
  installed: boolean;
  enabled: boolean;
};
type Install = { status: string; exitCode?: number; lines?: string[] };
export function AgentConnectionCatalog({
  providerId,
  agentId,
  onClose,
}: {
  providerId: string;
  agentId: string;
  onClose: () => void;
}) {
  const revision=useRef(0);
  const [entries, setEntries] = useState<Entry[]>([]),
    [selected, setSelected] = useState<Entry | null>(null),
    [query, setQuery] = useState("");
  const [error, setError] = useState(""),
    [busy, setBusy] = useState(false),
    [install, setInstall] = useState<Install | null>(null);
  const call = <T,>(action: string, input: Record<string, unknown> = {}) =>
    window.bridge!.agentCall<T>(providerId, `addons.mcp.${action}`, {
      agentId,
      ...input,
    });
  useEffect(() => {
    let cancelled = false;
    setBusy(true);
    void call<{ entries: Entry[] }>("catalog")
      .then((r) => {
        if (!cancelled) setEntries(r.entries);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      })
      .finally(() => {
        if (!cancelled) setBusy(false);
      });
    return () => {
      cancelled = true;
    };
  }, [providerId, agentId]);
  useEffect(() => {
    const request=++revision.current;
    setInstall(null);
    if (!selected) return;
    let cancelled = false;
    const poll = async () => {
      try {
        const r = await call<Install>("installStatus", { id: selected.name });
        if (cancelled || request!==revision.current) return;
        setInstall(r);
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    };
    void poll();
    return () => {
      cancelled = true;
    };
  }, [selected?.name]);
  useEffect(() => {
    if (!selected || install?.status !== "running") return;
    let cancelled = false;
    const timer = setTimeout(
      () =>
        void call<Install>("installStatus", { id: selected.name })
          .then((r) => {
            if (!cancelled) setInstall(r);
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
  }, [install, selected?.name]);
  useEffect(()=>{
    if(install?.status!=="installed"||!selected)return;
    setEntries(old=>old.map(e=>e.name===selected.name?{...e,installed:true}:e));
    setSelected(old=>old?{...old,installed:true}:old);
  },[install?.status,selected?.name]);
  return (
    <section className="agent-addon-panel">
      <header>
        <div>
          <h2>Connection catalog</h2>
          <p className="agent-muted">Available from the Hermes catalog.</p>
        </div>
        <button onClick={onClose}>Back to connections</button>
      </header>
      {error && <p role="alert">{error}</p>}
      <input
        aria-label="Search connection catalog"
        placeholder="Search connections…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />
      <div className="agent-resource-layout">
        <aside className="agent-connections-list">
          {entries
            .filter((e) =>
              `${e.name} ${e.description}`
                .toLowerCase()
                .includes(query.toLowerCase()),
            )
            .map((e) => (
              <button
                key={e.name}
                disabled={busy}
                onClick={() => {
                  setSelected(e);
                  setError("");
                }}
              >
                <strong>{e.name}</strong>
                <small>{e.installed ? "Installed" : e.transport}</small>
              </button>
            ))}
          {!entries.length && !busy && <p>No catalog entries available.</p>}
        </aside>
        <div className="agent-resource-editor">
          {selected ? (
            <>
              <h3>{selected.name}</h3>
              <p>{selected.description}</p>
              <p className="agent-muted">
                {selected.transport} ·{" "}
                {selected.authType || "No authentication"}
              </p>
              <details open>
                <summary>Connection details</summary>
                <pre>
                  {selected.url ||
                    [selected.command, ...selected.args]
                      .filter(Boolean)
                      .join(" ")}
                </pre>
                {selected.installUrl && (
                  <p>
                    Source: {selected.installUrl} {selected.installRef}
                  </p>
                )}
                {!!selected.bootstrap.length && (
                  <>
                    <h4>Setup commands</h4>
                    <pre>{selected.bootstrap.join("\n")}</pre>
                  </>
                )}
              </details>
              <form
                key={selected.name}
                onSubmit={async (e) => {
                  e.preventDefault();
                  const form = e.currentTarget,
                    data = new FormData(form),
                    env = Object.fromEntries(
                      selected.requiredEnv.map((field) => [
                        field.name,
                        String(data.get(field.name) || ""),
                      ]),
                    );
                  setBusy(true);
                  setError("");
                  try {
                    const result = await call<Install>("install", {
                      id: selected.name,
                      env,
                      enabled: true,
                    });
                    form.reset();
                    setInstall(result);
                  } catch (err) {
                    setError(err instanceof Error ? err.message : String(err));
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {selected.requiredEnv.map((field) => (
                  <label key={field.name}>
                    {field.prompt || field.name}
                    <input
                      type="password"
                      name={field.name}
                      autoComplete="off"
                      required={field.required}
                    />
                  </label>
                ))}
                <button
                  disabled={
                    busy ||
                    install?.status === "running" ||
                    selected.installed ||
                    install?.status === "installed"
                  }
                >
                  {install?.status === "running"
                    ? "Installing…"
                    : selected.installed || install?.status === "installed"
                      ? "Installed"
                      : "Install connection"}
                </button>
              </form>
              {install && install.status !== "none" && (
                <div role="status">
                  <strong>
                    {install.status === "installed"
                      ? "Connection installed"
                      : install.status === "failed"
                        ? "Installation failed"
                        : install.status === "unknown"
                          ? "Installation outcome unavailable"
                          : "Installation in progress"}
                  </strong>
                  {!!install.lines?.length && (
                    <details>
                      <summary>Installation log</summary>
                      <pre>{install.lines.join("\n")}</pre>
                    </details>
                  )}
                </div>
              )}
              {selected.postInstall && <p>{selected.postInstall}</p>}
            </>
          ) : (
            <p className="agent-muted">
              Choose a connection to review its setup and install it.
            </p>
          )}
        </div>
      </div>
    </section>
  );
}
