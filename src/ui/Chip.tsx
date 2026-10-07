import type { ReactNode } from "react";

/** Control/Chip: one pickable option, e.g. an answer to a question. */
export function Chip({
  selected = false,
  disabled,
  onClick,
  children,
}: {
  selected?: boolean;
  disabled?: boolean;
  onClick?: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      className={`ui-chip${selected ? " selected" : ""}`}
      aria-pressed={selected}
      disabled={disabled}
      title={typeof children === "string" ? children : undefined}
      onClick={onClick}
    >
      <span className="ui-chip-label">{children}</span>
    </button>
  );
}
