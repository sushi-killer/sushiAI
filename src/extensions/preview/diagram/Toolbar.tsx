import { GitCompare } from "lucide-react";
import type { Diagram } from "./types.ts";

/** The title chip with the counts of new, changed and removed blocks. */
export function Toolbar(props: { diagram: Diagram }) {
  const { diagram } = props;
  const count = (delta: string) =>
    diagram.nodes.filter((n) => n.delta === delta).length;
  const added = count("new");
  const changed = count("changed");
  const removed = count("removed");
  return (
    <div className="dg-toolbar">
      {diagram.title && (
        <span className="dg-chip title">
          <GitCompare size={12} aria-hidden />
          <span>{diagram.title}</span>
          {added > 0 && <b className="ok">+{added}</b>}
          {changed > 0 && <b className="warn">~{changed}</b>}
          {removed > 0 && <b className="bad">−{removed}</b>}
        </span>
      )}
    </div>
  );
}
