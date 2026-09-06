import { useEffect, useRef, useState } from "react";
import { ArrowUp, Square, Sparkles, CornerDownLeft } from "lucide-react";
import type { Panel } from "./types";
export function ChatPanel({
  panel,
  onSend,
  onCancel,
  onAgent,
}: {
  panel: Panel;
  onSend(text: string): void;
  onCancel(): void;
  onAgent(agent: string): void;
}) {
  const [draft, setDraft] = useState("");
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => {
    end.current?.scrollIntoView({ block: "end", behavior: "smooth" });
  }, [panel.messages, panel.busy]);
  function submit() {
    if (draft.trim() && !panel.busy) {
      onSend(draft.trim());
      setDraft("");
    }
  }
  return (
    <div className="chat">
      <div className="messages">
        {!panel.messages?.length ? (
          <div className="chat-empty">
            <Sparkles size={20} />
            <strong>A little context. A lot of possibility.</strong>
            <p>Ask a question or give your agent a task.</p>
          </div>
        ) : (
          panel.messages.map((message) => (
            <div key={message.id} className={`message ${message.role}`}>
              {message.role === "assistant" && (
                <span className="assistant-label">
                  <Sparkles size={11} />{" "}
                  {panel.agent === "codex" ? "Codex" : "Claude Code"}
                </span>
              )}
              <div>
                {message.text ||
                  (panel.busy ? (
                    <span className="thinking">
                      Thinking<span>…</span>
                    </span>
                  ) : (
                    ""
                  ))}
              </div>
            </div>
          ))
        )}
        {panel.error && (
          <div className="chat-error" role="alert">
            {panel.error}
          </div>
        )}
        <div ref={end} />
      </div>
      <div className="composer">
        <textarea
          aria-label="Message your agent"
          placeholder="Ask anything…"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (
              event.key === "Enter" &&
              !event.shiftKey &&
              !event.nativeEvent.isComposing
            ) {
              event.preventDefault();
              submit();
            }
          }}
        />
        <div className="composer-controls">
          <Sparkles size={12} />
          <select
            aria-label="Chat agent"
            disabled={panel.busy}
            value={panel.agent || "claude"}
            onChange={(event) => onAgent(event.target.value)}
          >
            <option value="claude">Claude Code</option>
            <option value="codex">Codex</option>
          </select>
          <span className="local-tag">Local CLI</span>
          {panel.busy ? (
            <button className="send" title="Stop response" onClick={onCancel}>
              <Square size={12} />
            </button>
          ) : (
            <button
              className={`send ${draft.trim() ? "enabled" : ""}`}
              aria-label="Send message"
              disabled={!draft.trim()}
              onClick={submit}
            >
              <ArrowUp size={17} />
            </button>
          )}
        </div>
      </div>
      <div className="chat-footnote">
        <CornerDownLeft size={10} /> to send · shift + enter for a new line
      </div>
    </div>
  );
}
