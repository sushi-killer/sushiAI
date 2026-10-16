import type { ReactNode } from "react";

/** Control/Chip: one pickable option, e.g. an answer to a question. */
export function Chip({
  selected = false,
  radio = false,
  disabled,
  onClick,
  className,
  children,
}: {
  selected?: boolean;
  /** One of a radio group: announced as checked, not as pressed. */
  radio?: boolean;
  disabled?: boolean;
  onClick?: () => void;
  /** An extra class for a variant, e.g. a secondary "Other…". */
  className?: string;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      className={`ui-chip${selected ? " selected" : ""}${className ? ` ${className}` : ""}`}
      {...(radio
        ? { role: "radio", "aria-checked": selected }
        : { "aria-pressed": selected })}
      disabled={disabled}
      title={typeof children === "string" ? children : undefined}
      onClick={onClick}
    >
      <span className="ui-chip-label">{children}</span>
    </button>
  );
}
