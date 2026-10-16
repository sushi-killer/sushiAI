/** A checkbox with its label. The label is the accessible name. */
export function Check({
  checked,
  label,
  disabled,
  onChange,
}: {
  checked: boolean;
  label: string;
  disabled?: boolean;
  onChange(checked: boolean): void;
}) {
  return (
    <input
      type="checkbox"
      className="ui-check"
      aria-label={label}
      checked={checked}
      disabled={disabled}
      onChange={(event) => onChange(event.target.checked)}
    />
  );
}
