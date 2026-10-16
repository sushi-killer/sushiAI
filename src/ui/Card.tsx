import type { ReactNode } from "react";

/** A bordered surface for one block of a page. `raised` is the lighter
 * surface of a detail or a chat next to a list. */
export function Card({
  as: Tag = "div",
  raised = false,
  className = "",
  label,
  children,
}: {
  as?: "div" | "section" | "article" | "aside";
  raised?: boolean;
  className?: string;
  label?: string;
  children: ReactNode;
}) {
  return (
    <Tag
      className={`ui-card${raised ? " raised" : ""} ${className}`.trim()}
      aria-label={label}
    >
      {children}
    </Tag>
  );
}

/** A small uppercase heading over a block, with an optional count in the
 * same line. */
export function Eyebrow({
  count,
  children,
}: {
  count?: ReactNode;
  children: ReactNode;
}) {
  return (
    <h3 className="ui-eyebrow">
      {children}
      {count !== undefined && <span className="ui-eyebrow-count">{count}</span>}
    </h3>
  );
}
