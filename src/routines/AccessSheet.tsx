import { Rows, Sheet, Tag } from "../ui/index.ts";
import type { AccessEntry } from "./types.ts";

/** R8: the tools a routine proposed and waits for. The grant belongs to this
 * routine only; the owner allows it or leaves it. */
export function AccessSheet({
  name,
  items,
  busy,
  onCancel,
  onAllow,
}: {
  name: string;
  items: AccessEntry[];
  busy: boolean;
  onCancel(): void;
  onAllow(): void;
}) {
  return (
    <Sheet
      title={`${name} needs access`}
      hint="Granted to this routine only. Each tool is shown exactly as it will run."
      onClose={onCancel}
      footer={
        <>
          <button
            type="button"
            className="ui-button ghost"
            disabled={busy}
            onClick={onCancel}
          >
            Cancel
          </button>
          <button
            type="button"
            className="ui-button primary"
            disabled={busy}
            onClick={onAllow}
          >
            Allow for this routine
          </button>
        </>
      }
    >
      <Rows as="ul" label="Tools">
        {items.map((item) => (
          <li key={item.tool} className="rt-access-row">
            <span className="rt-access-text">
              <strong>{item.label}</strong>
              {item.detail && <small>{item.detail}</small>}
              <code className="rt-access-tool">{item.tool}</code>
            </span>
            <Tag
              tone={item.kind === "write" ? "warning" : "neutral"}
              dot={false}
            >
              {item.kind}
            </Tag>
          </li>
        ))}
      </Rows>
      <p className="ui-sheet-hint rt-access-note">
        The agent never sees your password. Every write is in the run log.
      </p>
    </Sheet>
  );
}
