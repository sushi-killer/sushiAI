/** A row of mutually exclusive options in one frame (a radio group). */
export function Segmented<T extends string>({
  label,
  value,
  options,
  disabled,
  onChange,
}: {
  /** The accessible name of the group. */
  label: string;
  value: T;
  options: { id: T; label: string }[];
  disabled?: boolean;
  onChange(value: T): void;
}) {
  return (
    <span className="ui-segmented" role="radiogroup" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.id}
          type="button"
          role="radio"
          aria-checked={value === option.id}
          className={value === option.id ? "on" : ""}
          disabled={disabled}
          onClick={() => onChange(option.id)}
        >
          {option.label}
        </button>
      ))}
    </span>
  );
}
