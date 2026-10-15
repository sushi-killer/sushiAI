import { Dot } from "./Dot.tsx";

export type ListRowData = {
  key: string;
  title: string;
  meta: string;
  /** The meta line is an error. */
  danger?: boolean;
  /** A pulsing mark before the name: it is working now. */
  live?: boolean;
};

/** A titled group of selectable rows: "Needs you · 3" over a plain list. Each
 * row is a name and a muted line. Only the selected row has a background; the
 * hairline between two rows is left out next to it. */
export function ListGroup({
  label,
  rows,
  selected,
  onSelect,
}: {
  label: string;
  rows: ListRowData[];
  /** The key of the selected row, from any group. */
  selected: string | null;
  onSelect(key: string): void;
}) {
  return (
    <section className="ui-list-group" aria-label={label}>
      <h3 className="ui-list-head">{`${label} · ${rows.length}`}</h3>
      <ul className="ui-list">
        {rows.map((row, index) => {
          const before = rows[index - 1];
          const rule =
            before && selected !== row.key && selected !== before.key;
          return (
            <li key={row.key}>
              {rule && <hr className="ui-list-rule" />}
              <button
                type="button"
                className={`ui-list-row${selected === row.key ? " selected" : ""}`}
                aria-pressed={selected === row.key}
                onClick={() => onSelect(row.key)}
              >
                <strong>
                  {row.live && <Dot tone="info" live label="Running" />}
                  <span className="ui-list-title">{row.title}</span>
                </strong>
                <small className={row.danger ? "is-danger" : undefined}>
                  {row.meta}
                </small>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
