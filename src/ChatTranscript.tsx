import type { ReactNode } from "react";
import { useEffect, useRef } from "react";
import { FileText, FolderClosed, Image as ImageIcon, X } from "lucide-react";
import { RichText } from "./agents/AgentsView";
import "./ChatView.css";

export type TranscriptMessage = {
  id: string;
  role: "user" | "assistant";
  text: string;
  attachments?: string[];
  /** Which model answered - shown only when `renderModelLabel` says so, e.g.
   * once a thread has been answered by more than one model. */
  model?: string;
};

const basename = (p: string) => p.split("/").filter(Boolean).at(-1) || p;
const isImage = (p: string) => /\.(png|jpe?g|gif|webp)$/i.test(p);
const isFolder = (p: string) =>
  p.endsWith("/") || !/\.[^/]+$/.test(basename(p));

export function Attachment({
  path,
  onRemove,
}: {
  path: string;
  onRemove?: () => void;
}) {
  const Icon = isImage(path)
    ? ImageIcon
    : isFolder(path)
      ? FolderClosed
      : FileText;
  return (
    <span className="attachment" title={path}>
      <Icon size={12} />
      <span>{basename(path)}</span>
      {onRemove && (
        <button aria-label={`Remove ${basename(path)}`} onClick={onRemove}>
          <X size={11} />
        </button>
      )}
    </span>
  );
}

/** The scrollable body of a chat pane: a welcome screen until the first
 * message, then the message column with its trailing progress/error rows -
 * exactly what ChatView's main chat and the orchestrator's chat both need,
 * so neither hand-rolls its own copy of this markup. */
export function ChatTranscript({
  messages,
  busy,
  note,
  error,
  welcome,
  columnKey,
  renderModelLabel,
}: {
  messages: readonly TranscriptMessage[] | undefined;
  busy?: boolean;
  note?: string;
  error?: string;
  welcome: ReactNode;
  columnKey?: string;
  renderModelLabel?: (message: TranscriptMessage) => ReactNode;
}) {
  const end = useRef<HTMLDivElement>(null);

  useEffect(() => {
    end.current?.scrollIntoView({ block: "end" });
  }, [messages, busy, note]);

  const errorRow = error && (
    <div className="chat-error" role="alert">
      {error}
    </div>
  );

  return (
    <div className="chat-scroll">
      {!messages?.length ? (
        // A failure before the first message (a thread that can't load) has
        // no column to sit under, so it shows beneath the welcome instead.
        <div className="chat-welcome" key="welcome">
          {welcome}
          {errorRow}
        </div>
      ) : (
        <div className="chat-column" key={columnKey}>
          {messages.map((message) => (
            <div key={message.id} className={`message ${message.role}`}>
              {message.role === "assistant" ? (
                message.text ? (
                  <>
                    {renderModelLabel?.(message)}
                    <RichText text={message.text} />
                  </>
                ) : null
              ) : (
                <>
                  {message.text}
                  {message.attachments?.length ? (
                    <div className="chat-attachments">
                      {message.attachments.map((path) => (
                        <Attachment key={path} path={path} />
                      ))}
                    </div>
                  ) : null}
                </>
              )}
            </div>
          ))}
          {/* Codex often answers, then keeps working; progress belongs
              under the thread, not inside an empty bubble. */}
          {busy && (
            <div className="chat-progress">
              <i />
              <span className="thinking">
                {note || "Thinking"}
                <span>…</span>
              </span>
            </div>
          )}
          {errorRow}
          <div ref={end} />
        </div>
      )}
    </div>
  );
}
