import type { ReactNode } from "react";

/** Radio cards: one is chosen, each has a title and a line under it. */
export function ChoiceGroup<T extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: { id: T; title: string; detail: ReactNode }[];
  onChange(value: T): void;
}) {
  return (
    <div className="ui-choices" role="radiogroup" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.id}
          type="button"
          role="radio"
          aria-checked={value === option.id}
          className={`ui-choice${value === option.id ? " on" : ""}`}
          onClick={() => onChange(option.id)}
        >
          <strong>
            <span className="ui-radio" aria-hidden />
            {option.title}
          </strong>
          <small>{option.detail}</small>
        </button>
      ))}
    </div>
  );
}
