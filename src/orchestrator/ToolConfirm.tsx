import { useState } from "react";
import { useOrchestratorClient } from "./hostContext";
import { errorText } from "./helpers";
import { actionOutcome, parseArgsText, payloadPreview } from "./toolsModel";
import type { Tone } from "./helpers";
import type { ChatAction, ChatActionState } from "./types";
import { Tag } from "./ui";

const TAGS: Record<ChatActionState, { tone: Tone; text: string }> = {
  pending: { tone: "warning", text: "needs your OK" },
  sending: { tone: "info", text: "sending" },
  sent: { tone: "ok", text: "sent" },
  failed: { tone: "danger", text: "failed" },
  declined: { tone: "neutral", text: "not sent" },
};

/** Orch/ToolConfirm: a write the orchestrator wants to make on a connected
 * tool. Nothing runs until Send; Edit changes the arguments first and Don't
 * send refuses. What happened is kept by orchd on the message, so the card
 * reads the same after a reload. */
export function ToolConfirm({
  cwd,
  messageId,
  action,
  disabled,
}: {
  cwd: string;
  messageId: string;
  action: ChatAction;
  /** A reply is streaming: orchd refuses to run a call meanwhile. */
  disabled: boolean;
}) {
  const client = useOrchestratorClient();
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(() => payloadPreview(action.args));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const outcome = actionOutcome(action);
  const off = busy || disabled;

  async function send() {
    let args: Record<string, unknown> | undefined;
    if (editing) {
      const parsed = parseArgsText(text);
      if ("error" in parsed) {
        setError(parsed.error);
        return;
      }
      args = parsed.args;
    }
    await run(() => client.chatActionSend(cwd, messageId, args));
  }

  async function run(call: () => Promise<unknown>) {
    if (off) return;
    setBusy(true);
    setError("");
    try {
      await call();
      setEditing(false);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section
      className={`ochat-confirm ${action.state}`}
      aria-label="Needs your OK"
    >
      <header className="ochat-confirm-head">
        <Tag tone={TAGS[action.state].tone}>{TAGS[action.state].text}</Tag>
        <span className="ochat-confirm-title">
          {action.summary || action.tool}
        </span>
        {action.target && (
          <span className="ochat-confirm-target">{action.target}</span>
        )}
      </header>
      <p className="ochat-confirm-tool">
        {action.server} · {action.tool}
      </p>
      {editing ? (
        <textarea
          className="ochat-confirm-edit"
          aria-label="Arguments"
          spellCheck={false}
          value={text}
          onChange={(event) => setText(event.target.value)}
        />
      ) : (
        <pre className="ochat-confirm-payload">
          {payloadPreview(action.args)}
        </pre>
      )}
      {error && (
        <p className="ochat-error" role="alert">
          {error}
        </p>
      )}
      {outcome ? (
        <p
          className={`ochat-confirm-outcome ${action.state}`}
          role={action.state === "failed" ? "alert" : undefined}
        >
          {outcome}
        </p>
      ) : (
        <footer className="ochat-confirm-foot">
          <button
            type="button"
            className="ui-button primary"
            disabled={off}
            onClick={() => void send()}
          >
            Send
          </button>
          <button
            type="button"
            className="ui-button secondary"
            disabled={off}
            aria-pressed={editing}
            onClick={() => {
              setText(payloadPreview(action.args));
              setEditing(!editing);
            }}
          >
            Edit
          </button>
          <button
            type="button"
            className="ui-button secondary"
            disabled={off}
            onClick={() =>
              void run(() => client.chatActionDecline(cwd, messageId))
            }
          >
            Don't send
          </button>
          <span className="ochat-confirm-hint">reads never ask</span>
        </footer>
      )}
    </section>
  );
}
