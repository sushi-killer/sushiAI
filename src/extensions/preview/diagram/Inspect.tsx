import { MessageSquare, Trash2 } from "lucide-react";
import { useEffect, useRef } from "react";
import { label } from "./icons.ts";
import { presetOf } from "./model.ts";
import { ICONS, SHAPES } from "./types.ts";
import type { Diagram, DiagramOp, Picked } from "./types.ts";

type Props = {
  diagram: Diagram;
  picked: Picked;
  /** Focus the main text field on open (a note just added). */
  focusText?: boolean;
  onOp: (op: DiagramOp) => void;
  onDelete: () => void;
  onComment: (quote: string, where: string) => void;
};

export const ICON_OPTIONS: [string, string][] = [
  ["", "None"],
  ...ICONS.map((v): [string, string] => [v, label(v)]),
];

type SetField = Extract<DiagramOp, { op: "set" }>["field"];

export function nameOf(diagram: Diagram, id: string): string {
  return diagram.nodes.find((node) => node.id === id)?.title || id;
}

/** What a comment on this item quotes back to the agent. */
export function quoteOf(diagram: Diagram, picked: Picked): string {
  if (picked.type === "node")
    return diagram.nodes.find((n) => n.id === picked.id)?.title || picked.id;
  if (picked.type === "group")
    return diagram.groups.find((g) => g.id === picked.id)?.label || picked.id;
  if (picked.type === "note")
    return diagram.notes.find((n) => n.id === picked.id)?.text || picked.id;
  const edge = diagram.edges.find((e) => e.id === picked.id);
  if (!edge) return picked.id;
  const link = `${nameOf(diagram, edge.from)} → ${nameOf(diagram, edge.to)}`;
  return edge.label ? `${link} · ${edge.label}` : link;
}

function TextField(props: {
  label: string;
  value: string;
  autoFocus?: boolean;
  onCommit: (value: string) => void;
}) {
  const ref = useRef<HTMLInputElement>(null);
  const shown = useRef(props.value);
  shown.current = props.value;
  const commit = useRef(props.onCommit);
  commit.current = props.onCommit;
  // The file may change under the field; typing in it is never overwritten.
  useEffect(() => {
    const el = ref.current;
    if (el && document.activeElement !== el) el.value = props.value;
  }, [props.value]);
  // Switching the selection writes what was typed, to the item it was typed in.
  useEffect(() => {
    const el = ref.current;
    return () => {
      if (el && el.value !== shown.current) commit.current(el.value);
    };
  }, []);
  return (
    <label className="dg-field">
      <span>{props.label}</span>
      <input
        className="ui-input"
        ref={ref}
        defaultValue={props.value}
        spellCheck={false}
        autoFocus={props.autoFocus}
        onFocus={(event) => props.autoFocus && event.currentTarget.select()}
        onBlur={(event) => {
          if (event.target.value !== shown.current)
            props.onCommit(event.target.value);
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter") event.currentTarget.blur();
          if (event.key === "Escape") {
            event.currentTarget.value = props.value;
            event.currentTarget.blur();
          }
        }}
      />
    </label>
  );
}

export const MIXED = "\u0000mixed";

export function SelectField(props: {
  label: string;
  value: string;
  options: [string, string][];
  onCommit: (value: string) => void;
}) {
  return (
    <label className="dg-field">
      <span>{props.label}</span>
      <select
        className="ui-select"
        value={props.value}
        onChange={(event) => props.onCommit(event.target.value)}
      >
        {props.value === MIXED && (
          <option value={MIXED} disabled hidden>
            Mixed
          </option>
        )}
        {props.options.map(([value, text]) => (
          <option key={value} value={value}>
            {text}
          </option>
        ))}
      </select>
    </label>
  );
}

/** The right-hand panel while something is selected. Each change is one `set` op. */
export function Inspect({
  diagram,
  picked,
  focusText,
  onOp,
  onDelete,
  onComment,
}: Props) {
  const node =
    picked.type === "node"
      ? diagram.nodes.find((n) => n.id === picked.id)
      : undefined;
  const edge =
    picked.type === "edge"
      ? diagram.edges.find((e) => e.id === picked.id)
      : undefined;
  const note =
    picked.type === "note"
      ? diagram.notes.find((n) => n.id === picked.id)
      : undefined;
  const group =
    picked.type === "group"
      ? diagram.groups.find((g) => g.id === picked.id)
      : undefined;
  const set = (field: SetField, value: string) =>
    onOp({ op: "set", id: picked.id, field, value });
  const proposed = node?.status === "proposed" || edge?.status === "proposed";
  return (
    <aside className="dg-inspect" aria-label="Inspect">
      <div className="dg-inspect-head">
        <h2>Inspect</h2>
      </div>
      <div className="dg-inspect-body">
        {node && (
          <>
            <TextField
              label="Title"
              value={node.title}
              onCommit={(v) => v.trim() && set("title", v.trim())}
            />
            <SelectField
              label="Shape"
              value={node.shape || "box"}
              options={SHAPES.map((v): [string, string] => [v, label(v)])}
              onCommit={(v) => set("shape", v)}
            />
            <SelectField
              label="Icon"
              value={node.icon || ""}
              options={ICON_OPTIONS}
              onCommit={(v) => set("icon", v)}
            />
            <TextField
              label="Caption"
              value={node.caption ?? presetOf(node).caption ?? ""}
              onCommit={(v) => set("caption", v.trim())}
            />
            <TextField
              label="Subtitle"
              value={node.tech || ""}
              onCommit={(v) => set("tech", v.trim())}
            />
            <TextField
              label="Step"
              value={node.step === undefined ? "" : String(node.step)}
              onCommit={(v) => /^\d*$/.test(v.trim()) && set("step", v.trim())}
            />
            <SelectField
              label="Delta"
              value={node.delta || ""}
              options={[
                ["", "None"],
                ["new", "New"],
                ["changed", "Changed"],
                ["exists", "Exists"],
                ["removed", "Removed"],
              ]}
              onCommit={(v) => set("delta", v)}
            />
            <SelectField
              label="Group"
              value={node.group || ""}
              options={[
                ["", "None"],
                ...diagram.groups.map((g): [string, string] => [g.id, g.label]),
              ]}
              onCommit={(v) => set("group", v)}
            />
          </>
        )}
        {edge && (
          <>
            <div className="dg-field">
              <span>Link</span>
              <div className="dg-static">
                {nameOf(diagram, edge.from)} → {nameOf(diagram, edge.to)}
              </div>
            </div>
            <TextField
              label="Label"
              value={edge.label || ""}
              onCommit={(v) => set("label", v.trim())}
            />
            <SelectField
              label="Style"
              value={edge.style || "plain"}
              options={[
                ["plain", "Plain"],
                ["dashed", "Dashed"],
                ["guess", "Guess"],
              ]}
              onCommit={(v) => set("style", v === "plain" ? "" : v)}
            />
            <SelectField
              label="Arrows"
              value={edge.arrow || "end"}
              options={[
                ["end", "Forward"],
                ["both", "Both ways"],
                ["none", "None"],
              ]}
              onCommit={(v) => set("arrow", v === "end" ? "" : v)}
            />
          </>
        )}
        {note && (
          <TextField
            label="Text"
            value={note.text}
            autoFocus={focusText}
            onCommit={(v) => v.trim() && set("text", v.trim())}
          />
        )}
        {group && (
          <>
            <TextField
              label="Label"
              value={group.label}
              onCommit={(v) => v.trim() && set("label", v.trim())}
            />
            <SelectField
              label="Icon"
              value={group.icon || ""}
              options={ICON_OPTIONS}
              onCommit={(v) => set("icon", v)}
            />
            <div className="dg-field">
              <span>Members</span>
              <div className="dg-static">
                {diagram.nodes
                  .filter((n) => n.group === group.id)
                  .map((n) => n.title)
                  .join(", ") || "None"}
              </div>
            </div>
          </>
        )}
        {proposed && (
          <div className="dg-inspect-row">
            <button
              type="button"
              className="ui-button primary"
              onClick={() => onOp({ op: "accept", id: picked.id })}
            >
              Accept
            </button>
            <button
              type="button"
              className="ui-button secondary"
              onClick={() => onOp({ op: "reject", id: picked.id })}
            >
              Reject
            </button>
          </div>
        )}
        <div className="dg-inspect-row">
          <button
            type="button"
            className="ui-button primary"
            onClick={() =>
              onComment(quoteOf(diagram, picked), `${picked.type} ${picked.id}`)
            }
          >
            <MessageSquare size={12} aria-hidden />
            Comment
          </button>
          <button
            type="button"
            className="ui-button ghost dg-delete"
            onClick={onDelete}
          >
            <Trash2 size={12} aria-hidden />
            Delete
          </button>
        </div>
      </div>
    </aside>
  );
}
