import { MessageSquare, Send } from "lucide-react";
import { useEffect, useRef, useState, type RefObject } from "react";
import type { Comment } from "./artifact.ts";

export type Selection = {
  quote: string;
  /** The selected passage in a Markdown doc, marked while its box is open. */
  range?: Range;
  where?: string;
  pin?: { x: number; y: number };
  x: number;
  y: number;
};

/** The text the owner selected inside `ref`, with a spot next to it (relative
 * to `ref`'s own box) for the "Comment" button. */
export function useDocSelection(ref: RefObject<HTMLElement | null>) {
  const [selection, setSelection] = useState<Selection | null>(null);
  useEffect(() => {
    const root = ref.current;
    if (!root) return;
    const read = () => {
      const picked = window.getSelection();
      const quote = picked?.toString().trim() || "";
      if (!picked || picked.isCollapsed || !quote || !picked.rangeCount)
        return setSelection(null);
      // A triple-click on the last paragraph can end in the comment tray
      // below the document: keep what starts inside and clip the rest.
      const range = picked.getRangeAt(0).cloneRange();
      if (!root.contains(range.startContainer)) return setSelection(null);
      if (!root.contains(range.endContainer)) {
        const all = document.createRange();
        all.selectNodeContents(root);
        range.setEnd(all.endContainer, all.endOffset);
      }
      const clipped = range.toString().trim();
      if (!clipped) return setSelection(null);
      const box = root.getBoundingClientRect();
      const end = range.getBoundingClientRect();
      setSelection({
        quote: clipped,
        range,
        x: Math.max(0, Math.min(end.right - box.left, box.width - 90)),
        y: end.bottom - box.top + 6,
      });
    };
    const onUp = () => window.setTimeout(read, 0);
    root.addEventListener("mouseup", onUp);
    root.addEventListener("keyup", onUp);
    return () => {
      root.removeEventListener("mouseup", onUp);
      root.removeEventListener("keyup", onUp);
    };
  }, [ref]);
  return [selection, setSelection] as const;
}

export function CommentButton({
  selection,
  onOpen,
}: {
  selection: Selection;
  onOpen(): void;
}) {
  return (
    <button
      type="button"
      className="pv-comment-btn ui-button secondary"
      style={{ left: selection.x, top: selection.y }}
      // Keep the text selected while the button is pressed.
      onMouseDown={(event) => event.preventDefault()}
      onClick={onOpen}
    >
      <MessageSquare size={12} aria-hidden />
      Comment
    </button>
  );
}

export function CommentBox({
  selection,
  onAdd,
  onCancel,
}: {
  selection: Selection;
  onAdd(comment: Comment): void;
  onCancel(): void;
}) {
  const [note, setNote] = useState("");
  const area = useRef<HTMLTextAreaElement>(null);
  useEffect(() => area.current?.focus(), []);
  const add = () => {
    if (note.trim())
      onAdd({
        quote: selection.quote || undefined,
        where: selection.where || undefined,
        pin: selection.pin,
        note: note.trim(),
      });
  };
  return (
    <div
      className="pv-comment-box"
      style={{ left: selection.x, top: selection.y }}
      role="dialog"
      aria-label="Add a comment"
    >
      {(selection.quote || selection.where) && (
        <div className="pv-comment-quote">
          {selection.where && <b>{selection.where}</b>} {selection.quote}
        </div>
      )}
      <textarea
        ref={area}
        value={note}
        rows={2}
        aria-label="Comment"
        placeholder="Write a comment"
        onChange={(event) => setNote(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.stopPropagation();
            onCancel();
          } else if (event.key === "Enter" && !event.shiftKey) {
            event.preventDefault();
            add();
          }
        }}
      />
      <div className="pv-comment-foot">
        <span>Enter to add · Esc to cancel</span>
        <button
          type="button"
          className="ui-button primary"
          disabled={!note.trim()}
          onClick={add}
        >
          Add
        </button>
      </div>
    </div>
  );
}

const plural = (count: number) => `${count} comment${count === 1 ? "" : "s"}`;

/** The bottom bar, shown only while there are comments: how many, the first
 * quote, Clear, and the Send that turns them into one message to the agent. */
export function CommentTray({
  comments,
  agent,
  disabledReason,
  sending,
  error,
  onClear,
  onSend,
}: {
  comments: Comment[];
  agent: string;
  /** Why Send is off (no agent pane beside), or empty. */
  disabledReason: string;
  sending: boolean;
  error: string;
  onClear(): void;
  onSend(comments: Comment[]): void;
}) {
  if (!comments.length) return null;
  const first = comments[0].quote || comments[0].note;
  return (
    <div className="pv-tray">
      {error && (
        <div className="pd-alert" role="alert">
          {error}
        </div>
      )}
      <div className="pv-tray-row">
        <MessageSquare size={14} aria-hidden />
        <span className="pv-tray-count">{plural(comments.length)}</span>
        <span className="pv-tray-first">
          {"\u201C"}
          {first}
          {"\u201D"}
        </span>
        <button type="button" className="ui-button ghost" onClick={onClear}>
          Clear
        </button>
        <span title={disabledReason}>
          <button
            type="button"
            className="ui-button primary"
            disabled={!!disabledReason || sending}
            title={`Send to ${agent || "agent"}`}
            onClick={() => onSend(comments)}
          >
            <Send size={12} color="currentColor" aria-hidden />
            {sending ? "Sending…" : "Send"}
          </button>
        </span>
      </div>
    </div>
  );
}

/** Marks `range` with the `pv-anchor` highlight while it is set. */
export function useAnchorHighlight(range: Range | undefined) {
  useEffect(() => {
    if (!range || typeof CSS === "undefined" || !CSS.highlights) return;
    CSS.highlights.set("pv-anchor", new Highlight(range));
    return () => {
      CSS.highlights.delete("pv-anchor");
    };
  }, [range]);
}
