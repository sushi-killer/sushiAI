import { TriangleAlert } from "lucide-react";
import { useRef } from "react";
import { iconFor } from "./icons.ts";
import { noteSize } from "./model.ts";
import type { DiagramGroup, DiagramNote, Point, Rect } from "./types.ts";

export function RenameInput(props: {
  label: string;
  value: string;
  onCommit: (value: string) => void;
  onCancel: () => void;
}) {
  const done = useRef(false);
  const finish = (value: string | null) => {
    if (done.current) return;
    done.current = true;
    if (value === null) props.onCancel();
    else props.onCommit(value);
  };
  return (
    <input
      className="dg-rename"
      aria-label={props.label}
      autoFocus
      spellCheck={false}
      defaultValue={props.value}
      onFocus={(event) => event.currentTarget.select()}
      onBlur={(event) => finish(event.target.value)}
      onKeyDown={(event) => {
        event.stopPropagation();
        if (event.key === "Enter") finish(event.currentTarget.value);
        if (event.key === "Escape") finish(null);
      }}
    />
  );
}

export function GroupBox(props: {
  group: DiagramGroup;
  rect: Rect;
  selected: boolean;
  renaming: boolean;
  onRename: (label: string | null) => void;
}) {
  const { group, rect, selected } = props;
  const Icon = iconFor(group.icon);
  return (
    <>
      <div
        className="dg-group-head"
        style={{ left: rect.x, top: rect.y, width: rect.w }}
        data-group={group.id}
        title={group.label}
      >
        {Icon && <Icon size={11} aria-hidden />}
        <span>{group.label}</span>
      </div>
      {props.renaming && (
        <div
          className="dg-group-rename"
          style={{ left: rect.x + 8, top: rect.y + 3 }}
        >
          <RenameInput
            label="Boundary label"
            value={group.label}
            onCommit={(value) => props.onRename(value.trim())}
            onCancel={() => props.onRename(null)}
          />
        </div>
      )}
      {selected && (
        <span
          className="dg-resize"
          style={{ left: rect.x + rect.w - 4, top: rect.y + rect.h - 4 }}
          data-resize={group.id}
          aria-label={`Resize ${group.label}`}
        />
      )}
    </>
  );
}

export function NotePin(props: {
  note: DiagramNote;
  at: Point;
  selected: boolean;
}) {
  const { w, h } = noteSize(props.note);
  return (
    <button
      type="button"
      className={`dg-note${props.selected ? " selected" : ""}`}
      style={{ left: props.at.x, top: props.at.y, width: w, minHeight: h }}
      data-note={props.note.id}
      aria-label={`note ${props.note.text}`}
    >
      <TriangleAlert size={13} aria-hidden />
      <span>{props.note.text}</span>
    </button>
  );
}
