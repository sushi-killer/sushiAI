import type { ReactNode } from "react";
import type { Tone } from "../helpers";

/** Control/Tag: a small toned pill with a leading dot. */
export function Tag({ tone, children }: { tone: Tone; children: ReactNode }) {
  return (
    <span className={`ui-tag ui-tone-${tone}`}>
      <span className="ui-tag-dot" />
      {children}
    </span>
  );
}
