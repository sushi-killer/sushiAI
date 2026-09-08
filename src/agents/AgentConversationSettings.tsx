import { useEffect, useId, useState } from "react";
export function AgentConversationSettings({
  providerId,
  agentId,
  conversationId,
  disabled,
}: {
  providerId: string;
  agentId: string;
  conversationId: string;
  disabled: boolean;
}) {
  const [open, setOpen] = useState(false),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [notice, setNotice] = useState("");
  const [effort, setEffort] = useState("medium"),
    [model, setModel] = useState(""),
    [engine, setEngine] = useState("");
  const suggestionId = useId();
  const [currentProvider, setCurrentProvider] = useState("");
  const [options, setOptions] = useState<{ name: string; models: string[] }[]>(
    [],
  );
  const [optionsStatus, setOptionsStatus] = useState<
    "loading" | "ready" | "error"
  >("loading");
  const [confirmation, setConfirmation] = useState("");
  const call = <T,>(action: string, input: Record<string, unknown> = {}) =>
    window.bridge!.agentCall<T>(
      providerId,
      `conversations.settings.${action}`,
      { agentId, conversationId, ...input },
    );
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setOptionsStatus("loading");
    void call<{ providers: typeof options; defaultProvider?: string }>(
      "options",
    )
      .then((r) => {
        if (!cancelled) {
          setOptions(r.providers);
          setCurrentProvider((previous) =>
            !previous || previous === "unknown"
              ? r.defaultProvider || ""
              : previous,
          );
          setOptionsStatus("ready");
        }
      })
      .catch(() => {
        if (!cancelled) setOptionsStatus("error");
      });
    return () => {
      cancelled = true;
    };
  }, [open, providerId, agentId, conversationId]);
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
  const saveModel = async (confirm = false) => {
    const r = await call<{
      confirmRequired?: boolean;
      message?: string;
      warning?: string;
    }>("model", { model, provider: engine, confirm });
    if (r.confirmRequired) {
      setConfirmation(r.message || "Confirm model change.");
      return;
    }
    setConfirmation("");
    setNotice(
      ["Model saved for this conversation.", r.warning]
        .filter(Boolean)
        .join(" "),
    );
  };
  return (
    <div className="agent-conversation-settings">
      <button
        disabled={busy}
        type="button"
        aria-expanded={open}
        onClick={() => {
          if (open) {
            setOpen(false);
            return;
          }
          void run(async () => {
            const r = await call<{
              reasoning: string;
              model: string;
              provider: string;
            }>("read");
            setEffort(r.reasoning);
            setModel(r.model);
            setEngine("");
            setCurrentProvider(r.provider);
            setConfirmation("");
            setOpen(true);
          });
        }}
      >
        Conversation settings
      </button>
      {error && (
        <p role="alert" className="agent-error">
          {error}
        </p>
      )}
      {open && (
        <div className="agent-session-settings-fields">
          <p className="agent-muted">
            Applies to this conversation. Finish or stop a running turn before
            changing settings.
          </p>
          <fieldset disabled={busy || disabled}>
            <label>
              Conversation reasoning
              <select
                aria-label="Conversation reasoning"
                value={effort}
                onChange={(e) => setEffort(e.target.value)}
              >
                {[
                  "none",
                  "minimal",
                  "low",
                  "medium",
                  "high",
                  "xhigh",
                  "max",
                  "ultra",
                ].map((v) => (
                  <option key={v}>{v}</option>
                ))}
              </select>
            </label>
            <button
              type="button"
              onClick={() =>
                void run(async () => {
                  await call("reasoning", { value: effort });
                  setNotice("Reasoning saved for this conversation.");
                })
              }
            >
              Save conversation reasoning
            </button>
            <label>
              Model provider (optional)
              <input
                value={engine}
                list={`${suggestionId}-providers`}
                placeholder="Keep current provider"
                onChange={(e) => {
                  setEngine(e.target.value);
                  setConfirmation("");
                }}
              />
            </label>
            <label>
              Conversation model
              <input
                value={model}
                list={`${suggestionId}-models`}
                onChange={(e) => {
                  setModel(e.target.value);
                  setConfirmation("");
                }}
              />
            </label>
            <datalist id={`${suggestionId}-providers`}>
              {options
                .filter((p) => p.name !== currentProvider)
                .map((p) => (
                  <option key={p.name} value={p.name} />
                ))}
            </datalist>
            <datalist id={`${suggestionId}-models`}>
              {(
                options.find((p) => p.name === (engine || currentProvider))
                  ?.models || []
              ).map((id) => (
                <option key={id} value={id} />
              ))}
            </datalist>
            {confirmation ? (
              <>
                <p>{confirmation}</p>
                <button
                  type="button"
                  onClick={() => void run(() => saveModel(true))}
                >
                  Confirm conversation model
                </button>
                <button type="button" onClick={() => setConfirmation("")}>
                  Cancel model change
                </button>
              </>
            ) : (
              <button
                type="button"
                disabled={!model.trim()}
                onClick={() => void run(() => saveModel())}
              >
                Save conversation model
              </button>
            )}
          </fieldset>
          {optionsStatus === "loading" && (
            <p className="agent-muted">Loading model suggestions…</p>
          )}
          {optionsStatus === "error" && (
            <p className="agent-muted">
              Model suggestions are unavailable. You can still enter a model ID.
            </p>
          )}
          {notice && <p role="status">{notice}</p>}
        </div>
      )}
    </div>
  );
}
