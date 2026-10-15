import { ListGroup } from "../ui/index.ts";
import { entryKey, groupEntries, routineState, rowSubtitle } from "./model.ts";
import type { RoutineEntry } from "./types.ts";

/** The left column: routines in three groups, one plain row each - the name
 * and one muted line. Only the selected row has a background. */
export function RoutineList({
  entries,
  selected,
  onSelect,
}: {
  entries: RoutineEntry[];
  selected: string | null;
  onSelect(key: string): void;
}) {
  return (
    <aside className="rt-list" aria-label="Routines">
      {groupEntries(entries).map((group) => (
        <ListGroup
          key={group.id}
          label={group.label}
          selected={selected}
          onSelect={onSelect}
          rows={group.entries.map((entry) => {
            const state = routineState(entry);
            return {
              key: entryKey(entry),
              title: entry.routine.name,
              meta: rowSubtitle(entry),
              danger: state === "failed" || state === "paused",
              live: state === "running",
            };
          })}
        />
      ))}
    </aside>
  );
}
