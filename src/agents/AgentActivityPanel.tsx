import { useEffect, useState } from "react";
import type { AgentActivity } from "./types";
export function AgentActivityPanel({
  providerId,
  onOpen,
  onClose,
}: {
  providerId: string;
  onOpen: (activity: AgentActivity) => void;
  onClose: () => void;
}) {
  const [items, setItems] = useState<AgentActivity[]>([]),
    [error, setError] = useState("");
  useEffect(() => {
    let disposed = false;
    void window.bridge
      ?.agentCall<AgentActivity[]>(providerId, "activity.list")
      .then((rows) => {
        if (!disposed)
          setItems((old) =>
            [
              ...new Map(
                [...rows, ...old].map((row) => [row.id, row]),
              ).values(),
            ].slice(-500),
          );
      })
      .catch((e) => {
        if (!disposed) setError(String(e));
      });
    void window.bridge
      ?.agentCall<{ error: string | null }>(providerId, "activity.status")
      .then((status) => {
        if (!disposed && status.error) setError(status.error);
      })
      .catch(() => {});
    const off = window.bridge?.onAgents((event) => {
      if (event.providerId === providerId && event.type === "activity")
        setItems((old) =>
          [
            ...old.filter((i) => i.id !== (event.activity as AgentActivity).id),
            event.activity as AgentActivity,
          ].slice(-500),
        );
    });
    return () => {
      disposed = true;
      off?.();
    };
  }, [providerId]);
  return (
    <section className="agent-addon-panel">
      <header>
        <div>
          <h2>Agent activity</h2>
          <p className="agent-muted">
            Memory, skills and self-improvement — reported by your agents.
          </p>
        </div>
        <button onClick={onClose}>Back to chat</button>
      </header>
      {error && <p role="alert">{error}</p>}
      {!items.length && (
        <p className="agent-muted">
          When an agent saves a memory, changes a skill or completes a
          self-improvement review, it will appear here.
        </p>
      )}
      {[...items].reverse().map((item) => (
        <article className={`agent-activity-card ${item.kind}`} key={item.id}>
          <header>
            <strong>{item.title}</strong>
            <time
              dateTime={new Date(item.createdAt).toISOString()}
              title={new Date(item.createdAt).toLocaleString("en-US")}
            >
              {new Date(item.createdAt).toLocaleTimeString("en-US", {
                hour: "2-digit",
                minute: "2-digit",
              })}
            </time>
          </header>
          <small>
            {item.agentName} · {item.conversationTitle}
          </small>
          <p>{item.summary}</p>
          <button onClick={() => onOpen(item)}>Open conversation</button>
        </article>
      ))}
    </section>
  );
}
