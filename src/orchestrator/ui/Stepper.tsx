/** Control/Stepper: − value + for a small whole number. */
export function Stepper({
  value,
  min = 0,
  max = Number.POSITIVE_INFINITY,
  label,
  disabled,
  onChange,
}: {
  value: number;
  min?: number;
  max?: number;
  label: string;
  disabled?: boolean;
  onChange: (value: number) => void;
}) {
  return (
    <span className="ui-stepper" role="group" aria-label={label}>
      <button
        type="button"
        aria-label={`Decrease ${label}`}
        disabled={disabled || value <= min}
        onClick={() => onChange(value - 1)}
      >
        −
      </button>
      <span className="ui-stepper-value">{value}</span>
      <button
        type="button"
        aria-label={`Increase ${label}`}
        disabled={disabled || value >= max}
        onClick={() => onChange(value + 1)}
      >
        +
      </button>
    </span>
  );
}
