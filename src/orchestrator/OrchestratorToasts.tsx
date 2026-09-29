import { useEffect } from "react";
import { X } from "lucide-react";
import {
  toastLifetimeMs,
  type TaskTarget,
  type Toast,
  type ToastAction,
} from "./notices";

function ToastCard({
  toast,
  onOpen,
  onDismiss,
}: {
  toast: Toast;
  onOpen(): void;
  onDismiss(): void;
}) {
  const lifetime = toastLifetimeMs(toast);
  useEffect(() => {
    if (lifetime === null) return;
    const timer = setTimeout(onDismiss, lifetime);
    return () => clearTimeout(timer);
    // The timer belongs to this toast's id; a re-render never restarts it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [toast.id, lifetime]);
  return (
    <div
      className={`orch-toast ${toast.kind} ${lifetime === null ? "" : "timed"}`}
      role="status"
    >
      <img className="orch-toast-mascot" src="./sushi.svg" alt="" />
      <div className="orch-toast-text">
        <strong>{toast.title}</strong>
        <span title={toast.body}>{toast.body}</span>
      </div>
      <button className="orch-toast-open" onClick={onOpen}>
        Open
      </button>
      <button
        className="orch-toast-dismiss"
        aria-label="Dismiss"
        onClick={onDismiss}
      >
        <X size={13} />
      </button>
    </div>
  );
}

/** The bottom-right stack of orchd task notices (needs input, done, failed). */
export function OrchestratorToasts({
  toasts,
  dispatch,
  openTask,
}: {
  toasts: Toast[];
  dispatch(action: ToastAction): void;
  openTask(target: TaskTarget): void;
}) {
  if (!toasts.length) return null;
  return (
    <div className="orch-toasts">
      {toasts.map((toast) => (
        <ToastCard
          key={toast.id}
          toast={toast}
          onOpen={() => {
            openTask({
              taskId: toast.taskId,
              repo: toast.repo,
              focus: toast.focus,
            });
            dispatch({ type: "dismiss", id: toast.id });
          }}
          onDismiss={() => dispatch({ type: "dismiss", id: toast.id })}
        />
      ))}
    </div>
  );
}
