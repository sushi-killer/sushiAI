import {
  BoxSelect,
  Check,
  MessageSquare,
  Pencil,
  StickyNote,
  Trash2,
  X,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { SHAPE_ICON, iconFor } from "./icons.ts";
import { PRESETS } from "./model.ts";

export type Preset = (typeof PRESETS)[number];
export type AddChoice =
  { what: "block"; preset: Preset } | { what: "group" } | { what: "note" };

export type MenuItem =
  { separator: true } | { label: string; icon: LucideIcon; run: () => void };

export type MenuAt = { x: number; y: number; above?: boolean };

const item = (label: string, icon: LucideIcon, run: () => void): MenuItem => ({
  label,
  icon,
  run,
});
const sep: MenuItem = { separator: true };

/** The "add something" entries: the block presets, a group, a note. */
export function addItems(add: (choice: AddChoice) => void): MenuItem[] {
  return [
    ...PRESETS.flatMap((preset) => [
      item(
        preset.label,
        iconFor(preset.node.icon) ?? SHAPE_ICON[preset.node.shape ?? "box"],
        () => add({ what: "block", preset }),
      ),
      ...(preset.id === "decision" ? [sep] : []),
    ]),
    sep,
    item("Group", BoxSelect, () => add({ what: "group" })),
    item("Note", StickyNote, () => add({ what: "note" })),
  ];
}

export type Actions = {
  comment: () => void;
  accept?: () => void;
  reject?: () => void;
  onRename?: () => void;
  edit?: () => void;
  remove: () => void;
};

/** The entries for a selected thing; Rename or Edit text lead when it has one. */
export function thingItems(a: Actions): MenuItem[] {
  return [
    ...(a.onRename ? [item("Rename", Pencil, a.onRename)] : []),
    ...(a.edit ? [item("Edit text", Pencil, a.edit)] : []),
    item("Comment", MessageSquare, a.comment),
    ...(a.accept && a.reject
      ? [item("Accept", Check, a.accept), item("Reject", X, a.reject)]
      : []),
    sep,
    item("Delete", Trash2, a.remove),
  ];
}

const MARGIN = 8;

/** A right-click or "+" menu drawn at screen coordinates, kept on screen. */
export function ContextMenu(props: {
  at: MenuAt;
  items: MenuItem[];
  onClose: () => void;
}) {
  const { at, items, onClose } = props;
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: at.x, top: at.y });

  useLayoutEffect(() => {
    const box = ref.current?.getBoundingClientRect();
    if (!box) return;
    const top = at.above ? at.y - box.height - 4 : at.y;
    setPos({
      left: Math.max(
        MARGIN,
        Math.min(at.x, window.innerWidth - box.width - MARGIN),
      ),
      top: Math.max(
        MARGIN,
        Math.min(top, window.innerHeight - box.height - MARGIN),
      ),
    });
  }, [at]);

  useEffect(() => {
    const close = (event: Event) => {
      // The board pans by transform; its own scroll is focus scrolling.
      if (event.type === "scroll" && event.target instanceof HTMLElement)
        if (event.target.classList.contains("dg-board")) return;
      if (event.type === "keydown") {
        if ((event as KeyboardEvent).key === "Escape") onClose();
      } else if (
        event.type !== "mousedown" ||
        !ref.current?.contains(event.target as Node)
      )
        onClose();
    };
    const opts = { capture: true } as const;
    const events = ["mousedown", "keydown", "scroll", "wheel", "resize"];
    for (const name of events) window.addEventListener(name, close, opts);
    return () => {
      for (const name of events) window.removeEventListener(name, close, opts);
    };
  }, [onClose]);

  const onKeyDown = (event: React.KeyboardEvent) => {
    event.stopPropagation();
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const rows = [
      ...(ref.current?.querySelectorAll<HTMLElement>("[role=menuitem]") ?? []),
    ];
    const now = rows.indexOf(document.activeElement as HTMLElement);
    const step = event.key === "ArrowDown" ? 1 : -1;
    rows[(now + step + rows.length) % rows.length]?.focus();
  };

  let first = true;
  return (
    <div
      className="pv-menu dg-menu"
      role="menu"
      ref={ref}
      style={pos}
      onKeyDown={onKeyDown}
      onContextMenu={(event) => event.preventDefault()}
    >
      {items.map((it, i) => {
        if ("separator" in it)
          return <div key={i} className="dg-menu-sep" role="separator" />;
        const Icon = it.icon;
        const focus = first;
        first = false;
        return (
          <button
            key={it.label}
            type="button"
            role="menuitem"
            autoFocus={focus}
            onClick={() => {
              onClose();
              it.run();
            }}
          >
            <Icon size={12} aria-hidden />
            {it.label}
          </button>
        );
      })}
    </div>
  );
}
