import type { ReactNode } from "react";
import { ChevronDown } from "lucide-react";

/** The `.chip` control ChatView's composer toolbar pioneered - an icon and a
 * label that look static, with a full-size transparent `<select>` overlaid
 * on top so the native picker UI does the work. OrchestratorPanel's route
 * chip reuses this exact shape instead of a second implementation. */
export function ChipPicker({
  icon,
  label,
  value,
  options,
  ariaLabel,
  disabled,
  onChange,
}: {
  icon?: ReactNode;
  label: ReactNode;
  value: string;
  options: { value: string; label: string; title?: string }[];
  ariaLabel: string;
  disabled?: boolean;
  onChange: (value: string) => void;
}) {
  return (
    <label className="chip">
      {icon}
      <span>{label}</span>
      <ChevronDown size={11} />
      <select
        aria-label={ariaLabel}
        disabled={disabled}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      >
        {options.map((option) => (
          <option key={option.value} value={option.value} title={option.title}>
            {option.label}
          </option>
        ))}
      </select>
    </label>
  );
}
