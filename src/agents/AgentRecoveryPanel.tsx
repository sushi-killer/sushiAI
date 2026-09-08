import { useEffect, useState } from "react";
export function AgentRecoveryPanel({
  providerId,
  agentId,
  onClose,
}: {
  providerId: string;
  agentId: string;
  onClose: () => void;
}) {
  const [enabled, setEnabled] = useState(false),
    [saved, setSaved] = useState<boolean | null>(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [notice, setNotice] = useState("");
  const call = (action: string, input: Record<string, unknown> = {}) =>
    window.bridge!.agentCall<{ enabled: boolean }>(
      providerId,
      `addons.recovery.${action}`,
      { agentId, ...input },
    );
  useEffect(() => {
    let disposed = false;
    void call("read")
      .then((r) => {
        if (!disposed) {
          setEnabled(r.enabled);
          setSaved(r.enabled);
        }
      })
      .catch((e) => {
        if (!disposed) setError(String(e));
      });
    return () => {
      disposed = true;
    };
  }, [providerId, agentId]);
  return (
    <section className="agent-addon-panel">
      <header>
        <div>
          <h2>Recovery</h2>
          <p className="agent-muted">
            Choose what happens when an agent stops unexpectedly.
          </p>
        </div>
        <button onClick={onClose}>Back to chat</button>
      </header>
      {error && (
        <p role="alert" className="agent-error">
          {error}
        </p>
      )}
      {saved === null && !error && <p role="status">Loading recovery settings…</p>}
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          setBusy(true);
          setError("");
          setNotice("");
          try {
            const r = await call("update", { enabled });
            setEnabled(r.enabled);
            setSaved(r.enabled);
            setNotice("Recovery settings saved.");
          } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        <label>
          <input
            type="checkbox"
            checked={enabled}
            disabled={busy || saved === null}
            onChange={(e) => setEnabled(e.target.checked)}
          />{" "}
          Automatically continue interrupted tasks
        </label>
        <p className="agent-muted">
          When a crashed conversation is reopened, Hermes can continue
          unfinished work using its recovery limits. Disable this to continue
          manually.
        </p>
        <p className="agent-muted">
          Applies to this agent in sushiAI and other Hermes interfaces. Changing
          this setting does not stop a task already running.
        </p>
        <button
          type="submit"
          disabled={busy || saved === null || saved === enabled}
        >
          Save recovery settings
        </button>
      </form>
      {notice && <p role="status">{notice}</p>}
    </section>
  );
}
