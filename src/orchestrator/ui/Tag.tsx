import type { ReactNode } from "react";
import type { Tone } from "../helpers";

/** Control/Tag: a small toned pill, with a leading dot unless `dot` is off
 * (a plain label such as Brainstorm). */
export function Tag({
  tone,
  dot = true,
  children,
}: {
  tone: Tone;
  dot?: boolean;
  children: ReactNode;
}) {
  return (
    <span className={`ui-tag ui-tone-${tone}`}>
      {dot && <span className="ui-tag-dot" />}
      {children}
    </span>
  );
}
