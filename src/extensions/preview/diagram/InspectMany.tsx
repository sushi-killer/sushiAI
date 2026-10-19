import { MessageSquare, Trash2 } from "lucide-react";
import { label } from "./icons.ts";
import { ICON_OPTIONS, MIXED, SelectField, quoteOf } from "./Inspect.tsx";
import { SHAPES } from "./types.ts";
import type { Diagram, DiagramNode, Picked } from "./types.ts";

type NodeField = "shape" | "icon" | "delta" | "group";

const DELTAS: [string, string][] = [
  ["", "None"],
  ["new", "New"],
  ["changed", "Changed"],
  ["exists", "Exists"],
  ["removed", "Removed"],
];

const current = (node: DiagramNode, field: NodeField): string =>
  field === "shape" ? node.shape || "box" : node[field] || "";

/** What a comment on several things quotes back to the agent. */
export const quoteMany = (diagram: Diagram, picks: Picked[]): string =>
  picks.map((p) => quoteOf(diagram, p)).join(", ");

/** The right-hand panel with several things selected: the fields every
 * selected block shares, "Mixed" where they differ. */
export function InspectMany(props: {
  diagram: Diagram;
  picks: Picked[];
  onSet: (ids: string[], field: NodeField, value: string) => void;
  onComment: (quote: string, where: string) => void;
  onDelete: () => void;
}) {
  const { diagram, picks } = props;
  const ids = picks.filter((p) => p.type === "node").map((p) => p.id);
  const nodes = diagram.nodes.filter((n) => ids.includes(n.id));
  const shared = (field: NodeField) => {
    const values = new Set(nodes.map((n) => current(n, field)));
    return values.size === 1 ? [...values][0] : MIXED;
  };
  const field = (name: string, key: NodeField, options: [string, string][]) => (
    <SelectField
      label={name}
      value={shared(key)}
      options={options}
      onCommit={(v) => props.onSet(ids, key, v)}
    />
  );
  return (
    <aside className="dg-inspect" aria-label="Inspect">
      <div className="dg-inspect-head">
        <h2>{picks.length} selected</h2>
      </div>
      <div className="dg-inspect-body">
        {nodes.length > 0 && (
          <>
            {field(
              "Shape",
              "shape",
              SHAPES.map((v): [string, string] => [v, label(v)]),
            )}
            {field("Icon", "icon", ICON_OPTIONS)}
            {field("Delta", "delta", DELTAS)}
            {field("Group", "group", [
              ["", "None"],
              ...diagram.groups.map((g): [string, string] => [g.id, g.label]),
            ])}
          </>
        )}
        <div className="dg-inspect-row">
          <button
            type="button"
            className="ui-button primary"
            onClick={() =>
              props.onComment(
                quoteMany(diagram, picks),
                `${picks.length} selected`,
              )
            }
          >
            <MessageSquare size={12} aria-hidden />
            Comment
          </button>
          <button
            type="button"
            className="ui-button ghost dg-delete"
            onClick={props.onDelete}
          >
            <Trash2 size={12} aria-hidden />
            Delete
          </button>
        </div>
      </div>
    </aside>
  );
}
