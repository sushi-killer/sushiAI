import { useEffect, useState } from "react";
import {AgentSchedulerControl} from "./AgentSchedulerControl";
type Resource = { id: string; kind: string; data: Record<string, unknown> };
const string = (v: unknown) =>
  typeof v === "string" ? v : v == null ? "" : String(v);
export function AgentAddonPanel({
  providerId,
  agentId,
  addonId,
  onClose,
  onOpenConversation,
}: {
  providerId: string;
  agentId: string;
  addonId: string;
  onClose: () => void;
  onOpenConversation?: (id: string, title: string) => void;
}) {
  const [rows, setRows] = useState<Resource[]>([]),
    [selected, setSelected] = useState<Resource | null>(null);
  const [content, setContent] = useState(""),
    [original, setOriginal] = useState("");
  const [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [notice, setNotice] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false),
    [creating, setCreating] = useState(false);
  const [fields, setFields] = useState<Record<string, string>>({});
  const [reasoning,setReasoning]=useState("medium");
  const [models, setModels] = useState<
    { name: string; models: unknown[]; baseUrl?: string }[]
  >([]);
  const [runs, setRuns] = useState<
    {
      id: string;
      title?: string;
      started_at?: number;
      is_active?: boolean;
      message_count?: number;
    }[]
  >([]);
  const [trigger, setTrigger] = useState<{
    status: string;
    error?: string;
  } | null>(null);
  const [modelConfirm, setModelConfirm] = useState("");
  const call = async <T,>(
    action: string,
    input: Record<string, unknown> = {},
  ) =>
    window.bridge!.agentCall<T>(providerId, `addons.${addonId}.${action}`, {
      agentId,
      ...input,
    });
  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await fn();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const load = async () => {
    setSelected(null);
    setConfirmDelete(false);
    setCreating(false);
    if (["soul", "profile"].includes(addonId)) {
      const r = await call<Resource>("read");
      setSelected(r);
      setContent(string(r.data.content ?? r.data.description));
      setOriginal(string(r.data.content ?? r.data.description));
      return;
    }
    if (addonId === "models") {
      const current = await call<Resource>("current");
      setSelected(current);
      setFields({
        provider: string(current.data.provider),
        model: string(current.data.model),
      });
      const options = await call<{ providers: typeof models }>("options");
      setModels(options.providers);
      const effort=await call<{value:string}>("reasoning");setReasoning(effort.value);
      return;
    }
    const result = await call<{ resources: Resource[] }>(
      addonId === "memory" ? "graph" : "list",
    );
    setRows(result.resources);
  };
  useEffect(() => {
    void run(load);
  }, [providerId, agentId, addonId]);
  const choose = (r: Resource) =>
    void run(async () => {
      setConfirmDelete(false);
      setCreating(false);
      const full =
        addonId === "tools" ? r : await call<Resource>("read", { id: r.id });
      setSelected(full);
      setContent(string(full.data.content));
      setOriginal(string(full.data.content));
      if (addonId === "schedules") {
        const schedule = full.data.schedule as
          Record<string, unknown> | undefined;
        setFields({
          name: string(full.data.name),
          prompt: string(full.data.prompt),
          schedule: string(full.data.scheduleExpression || schedule?.expr),
          deliver: string(full.data.deliver),
          model: string(full.data.model),
          provider: string(full.data.provider),
        });
      }
    });
  useEffect(() => {
    setRuns([]);
    setTrigger(null);
    if (addonId !== "schedules" || !selected) return;
    let cancelled = false;
    const poll = async () => {
      try {
        const r = await call<{ status: string; error?: string }>(
          "triggerStatus",
          { id: selected.id },
        );
        if (cancelled) return;
        setTrigger(r);
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    };
    void poll();
    return () => {
      cancelled = true;
    };
  }, [selected?.id, addonId]);
  useEffect(() => {
    if (addonId !== "schedules" || !selected || trigger?.status !== "running")
      return;
    let cancelled = false;
    const timer = setTimeout(
      () =>
        void call<{ status: string; error?: string }>("triggerStatus", {
          id: selected.id,
        })
          .then((r) => {
            if (!cancelled) setTrigger(r);
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
  }, [trigger, selected?.id, addonId]);
  const save = () =>
    void run(async () => {
      if (addonId === "models") {
        const p = models.find((m) => m.name === fields.provider);
        const r = await call<{
          ok: boolean;
          confirmRequired?: boolean;
          confirmMessage?: string;
        }>("set", {
          provider: fields.provider,
          model: fields.model,
          ...(p?.baseUrl ? { baseUrl: p.baseUrl } : {}),
          confirm: !!modelConfirm,
        });
        if (r.confirmRequired) {
          setModelConfirm(r.confirmMessage || "Confirm this model change.");
          return;
        }
        setModelConfirm("");
      } else if (addonId === "schedules") {
        const value = Object.fromEntries(
          Object.entries(fields).filter(
            ([k, v]) =>
              !creating ||
              v !== "" ||
              ["model", "provider", "deliver"].includes(k),
          ),
        );
        await call(
          creating ? "create" : "update",
          creating ? value : { id: selected?.id, updates: value },
        );
        await load();
      } else if (addonId === "profile")
        await call("update", { description: content });
      else if (creating) {
        await call("create", {
          id: fields.name,
          content,
          ...(fields.category ? { category: fields.category } : {}),
        });
        await load();
      } else
        await call("update", {
          ...(addonId === "soul" ? {} : { id: selected?.id }),
          content,
          expectedContent: original,
        });
      setOriginal(content);
      setNotice(
        "Saved. Profile changes apply to new turns according to the agent's settings.",
      );
      if (addonId === "memory") await load();
    });
  const remove = () =>
    void run(async () => {
      await call(addonId === "skills" ? "archive" : "delete", {
        id: selected?.id,
        ...(["skills", "memory"].includes(addonId)
          ? { expectedContent: original }
          : {}),
      });
      await load();
      setNotice("Removed.");
    });
  const field = (name: string, label: string, multiline = false) => (
    <label key={name}>
      {label}
      {multiline ? (
        <textarea
          aria-label={label}
          value={fields[name] || ""}
          onChange={(e) => setFields((v) => ({ ...v, [name]: e.target.value }))}
        />
      ) : (
        <input
          aria-label={label}
          value={fields[name] || ""}
          onChange={(e) => setFields((v) => ({ ...v, [name]: e.target.value }))}
        />
      )}
    </label>
  );
  return (
    <section className="agent-addon-panel">
      <header>
        <h2>
          {addonId === "soul"
            ? "Instructions"
            : addonId.charAt(0).toUpperCase() + addonId.slice(1)}
        </h2>
        <div className="agent-actions">
          <button disabled={busy} onClick={() => void run(load)}>
            Refresh
          </button>
          {["skills", "schedules"].includes(addonId) && (
            <button
              onClick={() => {
                setCreating(true);
                setSelected(null);
                setFields({});
                setContent(
                  addonId === "skills"
                    ? "---\nname: new-skill\ndescription: Describe when to use this skill.\n---\n\n# Instructions\n"
                    : "",
                );
              }}
            >
              Create
            </button>
          )}
          <button onClick={onClose}>Back to chat</button>
        </div>
      </header>
      {error && (
        <p role="alert" className="agent-error">
          {error}
        </p>
      )}
      {notice && <p role="status">{notice}</p>}
      {addonId==="schedules"&&<AgentSchedulerControl providerId={providerId} agentId={agentId}/>}
      <div
        className={`agent-resource-layout ${["profile", "soul", "models"].includes(addonId) ? "single" : ""}`}
      >
        {!["profile", "soul", "models"].includes(addonId) && (
          <div className="agent-resource-list">
            {rows.map((r) => (
              <div className="agent-resource-row" key={r.id}>
                <button
                  className={selected?.id === r.id ? "active" : ""}
                  onClick={() => choose(r)}
                >
                  <strong>
                    {string(
                      r.data.title || r.data.label || r.data.name || r.id,
                    )}
                  </strong>
                  <small>
                    {string(
                      r.data.description ||
                        r.data.scheduleDisplay ||
                        r.data.source ||
                        r.data.provenance,
                    )}
                  </small>
                </button>
                {["skills", "tools"].includes(addonId) && (
                  <input
                    aria-label={`Enable ${r.id}`}
                    type="checkbox"
                    checked={r.data.enabled === true}
                    disabled={busy}
                    onChange={(e) =>
                      void run(async () => {
                        await call("toggle", {
                          id: r.id,
                          enabled: e.target.checked,
                        });
                        await load();
                      })
                    }
                  />
                )}
              </div>
            ))}
            {!rows.length && (
              <p className="agent-muted">
                {busy ? "Loading…" : "No resources yet."}
              </p>
            )}
          </div>
        )}
        <div className="agent-resource-editor">
          {selected || creating ? (
            <>
              {addonId === "models" ? (
                <>
                  <label>
                    Provider
                    <select
                      value={fields.provider || ""}
                      onChange={(e) => {
                        setFields((v) => ({ ...v, provider: e.target.value }));
                        setModelConfirm("");
                      }}
                    >
                      {models.map((p) => (
                        <option key={p.name}>{p.name}</option>
                      ))}
                    </select>
                  </label>
                  <label>
                    Model
                    <input
                      list="agent-model-options"
                      value={fields.model || ""}
                      onChange={(e) => {
                        setFields((v) => ({ ...v, model: e.target.value }));
                        setModelConfirm("");
                      }}
                    />
                    <datalist id="agent-model-options">
                      {models
                        .find((p) => p.name === fields.provider)
                        ?.models.map((m, i) => {
                          const id =
                            typeof m === "string"
                              ? m
                              : string((m as Record<string, unknown>).id);
                          return <option value={id} key={i} />;
                        })}
                    </datalist>
                  </label>
                  <label>Reasoning effort<select aria-label="Default reasoning effort" value={reasoning} onChange={e=>setReasoning(e.target.value)}>{["none","minimal","low","medium","high","xhigh","max","ultra"].map(value=><option key={value} value={value}>{value}</option>)}</select></label>
                  <button disabled={busy} onClick={()=>void run(async()=>{await call("setReasoning",{value:reasoning});setNotice("Reasoning effort saved for new conversations.");})}>Save reasoning effort</button>
                  {modelConfirm && <p>{modelConfirm}</p>}
                  <button disabled={busy} onClick={save}>
                    {modelConfirm ? "Confirm model change" : "Use model"}
                  </button>
                </>
              ) : addonId === "schedules" ? (
                <>
                  {field("name", "Name")}
                  {field("schedule", "Schedule expression")}
                  {field("prompt", "Instructions", true)}
                  {field("deliver", "Delivery destination (optional)")}
                  {field("model", "Model (optional)")}
                  {field("provider", "Provider (optional)")}
                  {selected && (
                    <p className="agent-muted">
                      {string(selected.data.state)} · Next run:{" "}
                      {string(selected.data.nextRunAt) || "Not scheduled"}
                    </p>
                  )}
                  <button disabled={busy} onClick={save}>
                    Save schedule
                  </button>
                  {!creating && (
                    <div className="agent-actions">
                      <button
                        disabled={busy || trigger?.status === "running"}
                        onClick={() =>
                          void run(async () =>
                            setTrigger(
                              await call("trigger", { id: selected?.id }),
                            ),
                          )
                        }
                      >
                        Run now
                      </button>
                      <button
                        disabled={busy}
                        onClick={() =>
                          void run(async () => {
                            const r = await call<{ runs: typeof runs }>(
                              "runs",
                              { id: selected?.id, limit: 100 },
                            );
                            setRuns(r.runs);
                            setNotice(
                              r.runs.length
                                ? "Latest runs loaded."
                                : "No runs yet.",
                            );
                          })
                        }
                      >
                        Run history
                      </button>
                      {["pause", "resume"].map((action) => (
                        <button
                          key={action}
                          disabled={busy}
                          onClick={() =>
                            void run(async () => {
                              await call(action, { id: selected?.id });
                              await load();
                            })
                          }
                        >
                          {action}
                        </button>
                      ))}
                      <button onClick={() => setConfirmDelete(true)}>
                        Delete
                      </button>
                    </div>
                  )}
                  {trigger && trigger.status !== "none" && (
                    <p role="status">
                      {trigger.status === "running"
                        ? "Running in Hermes…"
                        : trigger.status === "completed"
                          ? "Run completed"
                          : trigger.error || trigger.status}
                    </p>
                  )}
                  {runs.map((r) => (
                    <button
                      key={r.id}
                      disabled={!onOpenConversation}
                      onClick={() =>
                        onOpenConversation?.(r.id, r.title || "Scheduled run")
                      }
                    >
                      <strong>{r.title || "Scheduled run"}</strong>
                      <small>
                        {r.is_active ? "Active" : "Saved"} ·{" "}
                        {r.message_count || 0} messages
                      </small>
                    </button>
                  ))}
                </>
              ) : addonId === "tools" ? (
                <>
                  <h3>{string(selected?.data.label || selected?.id)}</h3>
                  <p>{string(selected?.data.description)}</p>
                  <p>{string(selected?.data.toolCount)} tools</p>
                </>
              ) : (
                <>
                  {creating && (
                    <>
                      {field("name", "Skill name")}
                      {field("category", "Category (optional)")}
                    </>
                  )}
                  <label>
                    {addonId === "profile" ? "Description" : "Content"}
                    <textarea
                      aria-label={
                        addonId === "profile" ? "Description" : "Content"
                      }
                      className="agent-content-editor"
                      spellCheck={false}
                      value={content}
                      onChange={(e) => setContent(e.target.value)}
                    />
                  </label>
                  <div className="agent-actions">
                    <button
                      disabled={busy || (!creating && content === original)}
                      onClick={save}
                    >
                      Save
                    </button>
                    {!creating && ["memory", "skills"].includes(addonId) && (
                      <button
                        disabled={busy}
                        onClick={() => setConfirmDelete(true)}
                      >
                        {addonId === "skills"
                          ? "Archive skill"
                          : "Delete memory"}
                      </button>
                    )}
                  </div>
                </>
              )}
              {confirmDelete && (
                <div className="agent-delete-confirm">
                  Remove this resource?
                  <button disabled={busy} onClick={remove}>
                    Confirm removal
                  </button>
                  <button onClick={() => setConfirmDelete(false)}>
                    Cancel
                  </button>
                </div>
              )}
            </>
          ) : (
            <p className="agent-muted">Select a resource to view it.</p>
          )}
        </div>
      </div>
    </section>
  );
}
