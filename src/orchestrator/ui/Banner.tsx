import type { ReactNode } from "react";

/** Control/Banner: a toned notice with a title, a detail line and one
 * action. */
export function Banner({
  tone,
  title,
  body,
  action,
}: {
  tone: "warning" | "danger" | "info";
  title: string;
  body?: ReactNode;
  action?: { label: string; onClick: () => void; disabled?: boolean };
}) {
  return (
    <div
      className={`ui-banner ui-tone-${tone}`}
      role={tone === "info" ? "status" : "alert"}
    >
      <span className="ui-banner-mark" />
      <span className="ui-banner-text">
        <span className="ui-banner-title">{title}</span>
        {body && <span className="ui-banner-body">{body}</span>}
      </span>
      {action && (
        <button
          type="button"
          className="ui-button secondary"
          disabled={action.disabled}
          onClick={action.onClick}
        >
          {action.label}
        </button>
      )}
    </div>
  );
}
