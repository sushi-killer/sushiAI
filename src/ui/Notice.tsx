import type { ReactNode } from "react";

/** A toned strip inside a card: one line of what needs attention, an optional
 * detail under it and an action on the right. */
export function Notice({
  tone = "warning",
  title,
  detail,
  action,
}: {
  tone?: "warning" | "danger";
  title: string;
  detail?: string;
  action?: ReactNode;
}) {
  return (
    <div className={`ui-notice ui-tone-${tone}`} role="status">
      <span className="ui-notice-text">
        <strong>{title}</strong>
        {detail && <small>{detail}</small>}
      </span>
      {action}
    </div>
  );
}
