import type { Tone } from "./tone.ts";

/** A 6px status dot, named for assistive tech. `live` pulses. */
export function Dot({
  tone,
  label,
  live = false,
}: {
  tone: Tone;
  label?: string;
  live?: boolean;
}) {
  return (
    <span
      className={`ui-dot ui-tone-${tone}${live ? " live" : ""}`}
      {...(label
        ? { role: "img", "aria-label": label }
        : { "aria-hidden": true })}
    />
  );
}
