import { useEffect, useState } from "react";
type Status = {
  enabled: boolean;
  status: string;
  error?: string | null;
  gatewayRunning?: boolean;
  gatewayState?: string;
};
export function AgentSchedulerControl({
  providerId,
  agentId,
}: {
  providerId: string;
  agentId: string;
}) {
  const [status, setStatus] = useState<Status | null>(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const call = <T,>(action: string, input: Record<string, unknown> = {}) =>
    window.bridge!.agentCall<T>(providerId, `addons.schedules.${action}`, {
      agentId,
      ...input,
    });
  useEffect(() => {
    let cancelled = false,
      timer: ReturnType<typeof setTimeout>;
    const refresh = async () => {
      try {
        const state = await call<Status>("schedulerStatus");
        if (!cancelled) {
          setStatus(state);
          setError("");
        }
      } catch (e) {
        if (!cancelled) setError(String(e));
      } finally {
        if (!cancelled) timer = setTimeout(refresh, 15000);
      }
    };
    void refresh();
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [providerId, agentId]);
  return (
    <div className="agent-scheduler-control">
      <div>
        <strong>Automatic schedules</strong>
        <p className="agent-muted">
          {status?.gatewayRunning
            ? "Hermes gateway is running."
            : "No running Hermes gateway detected for this profile."}{" "}
          {status?.enabled
            ? `sushiAI executor: ${status.status}.`
            : "sushiAI executor is off."}
        </p>
        <p className="agent-muted">
          The sushiAI executor handles all local Hermes profiles while this app
          is open. Existing gateways continue independently.
        </p>
      </div>
      <button
        disabled={busy || !status}
        onClick={async () => {
          setBusy(true);
          setError("");
          try {
            await call("schedulerSet", { enabled: !status?.enabled });
            setStatus(await call<Status>("schedulerStatus"));
          } catch (e) {
            setError(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        {busy
          ? "Updating…"
          : status?.enabled
            ? "Disable in sushiAI"
            : "Enable in sushiAI"}
      </button>
      {(error || status?.error) && <p role="alert">{error || status?.error}</p>}
    </div>
  );
}
