import type { ReactNode } from "react";

/** A muted line of facts under a title ("Fridays at 17:00 · 14 items"). It
 * wraps; `single` keeps one line and shortens it with an ellipsis. */
export function MetaLine({
  children,
  single = false,
  tone,
}: {
  children: ReactNode;
  single?: boolean;
  tone?: "danger";
}) {
  return (
    <p
      className={`ui-meta${single ? " single" : ""}${tone ? " is-danger" : ""}`}
    >
      {children}
    </p>
  );
}
