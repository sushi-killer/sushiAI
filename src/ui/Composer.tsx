import { useLayoutEffect, useRef } from "react";
import { composerHeight } from "./composerSize.ts";

/** A pill with a text field and one button inside on the right. The field
 * grows with what is typed up to six lines, then scrolls, and shrinks back;
 * the button stays at the bottom edge. Enter sends, Shift+Enter breaks the
 * line. */
export function Composer({
  value,
  label,
  placeholder,
  submitLabel,
  tone = "primary",
  compact = false,
  autoFocus = false,
  busy = false,
  busyLabel = "Starting…",
  onChange,
  onSubmit,
}: {
  value: string;
  /** The accessible name of the field. */
  label: string;
  placeholder: string;
  submitLabel: string;
  tone?: "primary" | "secondary";
  /** The smaller pill of a side chat. */
  compact?: boolean;
  /** Puts the caret in the field when the screen opens. */
  autoFocus?: boolean;
  busy?: boolean;
  busyLabel?: string;
  onChange(value: string): void;
  onSubmit(text: string): void;
}) {
  const field = useRef<HTMLTextAreaElement>(null);
  const send = () => {
    if (value.trim() && !busy) onSubmit(value.trim());
  };

  useLayoutEffect(() => {
    const node = field.current;
    if (!node) return;
    const fit = () => {
      // Back to the natural height first, so a shorter text shrinks the field.
      node.style.height = "auto";
      const style = getComputedStyle(node);
      const next = composerHeight(
        node.scrollHeight,
        parseFloat(style.lineHeight) || 16,
        parseFloat(style.paddingTop) + parseFloat(style.paddingBottom),
      );
      node.style.height = `${next.height}px`;
      node.style.overflowY = next.overflow ? "auto" : "hidden";
    };
    fit();
    // A narrower column wraps the same text onto more lines.
    if (typeof ResizeObserver === "undefined") return;
    let width = node.clientWidth;
    const observer = new ResizeObserver(() => {
      if (node.clientWidth === width) return;
      width = node.clientWidth;
      fit();
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, [value]);

  return (
    <form
      className={`ui-composer${compact ? " compact" : ""}`}
      onSubmit={(event) => {
        event.preventDefault();
        send();
      }}
    >
      <textarea
        ref={field}
        className="ui-composer-input"
        aria-label={label}
        placeholder={placeholder}
        rows={1}
        autoFocus={autoFocus}
        value={value}
        disabled={busy}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => {
          if (
            event.key === "Enter" &&
            !event.shiftKey &&
            !event.nativeEvent.isComposing
          ) {
            event.preventDefault();
            send();
          }
        }}
      />
      <button type="submit" className={`ui-button ${tone}`} disabled={busy}>
        {busy ? busyLabel : submitLabel}
      </button>
    </form>
  );
}
