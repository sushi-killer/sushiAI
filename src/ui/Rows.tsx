import type { ReactNode } from "react";

/** A bordered card of rows split by hairlines. Its children are the rows. */
export function Rows({
  as: Tag = "div",
  label,
  children,
}: {
  as?: "div" | "ul";
  label?: string;
  children: ReactNode;
}) {
  return (
    <Tag className="ui-rows" aria-label={label}>
      {children}
    </Tag>
  );
}

/** Key-value rows ("When", "Where", ...): a muted key and a value that
 * shortens with an ellipsis. */
export function KeyValue({
  label,
  children,
}: {
  label?: string;
  children: ReactNode;
}) {
  return <Rows label={label}>{children}</Rows>;
}

export function KeyValueRow({
  label,
  note,
  children,
}: {
  label: string;
  /** A remark under the value (a problem with it). */
  note?: string;
  children: ReactNode;
}) {
  return (
    <div className="ui-kv-row">
      <span className="ui-kv-label">{label}</span>
      <span className="ui-kv-value">
        {typeof children === "string" ? (
          <span className="ui-kv-text" title={children}>
            {children}
          </span>
        ) : (
          children
        )}
        {note && <span className="ui-kv-note">{note}</span>}
      </span>
    </div>
  );
}
