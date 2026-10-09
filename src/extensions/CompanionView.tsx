import { useEffect, useMemo, useState } from "react";
import { QrCode } from "./QrCode.tsx";
import {
  INITIAL_COMPANION_STATE,
  createCompanionController,
  listRows,
  rowBusyKey,
  type CompanionViewState,
} from "./companionModel.ts";
import type {
  CompanionValue,
  CompanionView as CompanionViewDescriptor,
  Tone,
} from "./types.ts";

const TONES: readonly Tone[] = [
  "neutral",
  "info",
  "warning",
  "danger",
  "muted",
  "ok",
];

function FieldValue({
  type,
  label,
  value,
}: {
  type: CompanionViewDescriptor["fields"][number]["type"];
  label: string;
  value: CompanionValue | undefined;
}) {
  if (value === null || value === undefined)
    return <span className="companion-empty">-</span>;
  if (type === "status" && typeof value === "object" && !Array.isArray(value)) {
    const tone = TONES.includes(value.tone) ? value.tone : "neutral";
    return (
      <span className="companion-status" data-tone={tone}>
        {value.text}
      </span>
    );
  }
  if (typeof value !== "string")
    return <span className="companion-empty">-</span>;
  if (type === "qr") return <QrCode text={value} label={label} />;
  return <span className="companion-text">{value}</span>;
}

function ListField({
  field,
  value,
  busy,
  onRow,
}: {
  field: CompanionViewDescriptor["fields"][number];
  value: CompanionValue | undefined;
  busy: string | null;
  onRow(fieldId: string, rowId: string): void;
}) {
  const rows = listRows(value);
  if (!rows.length) return <span className="companion-empty">-</span>;
  return (
    <ul className="companion-list" aria-label={field.label}>
      {rows.map((row) => {
        const rowBusy = busy === rowBusyKey(field.id, row.id);
        return (
          <li className="companion-row" key={row.id}>
            <span className="companion-row-main">
              <span className="companion-row-label">{row.label}</span>
              {row.detail && (
                <span className="companion-row-detail">{row.detail}</span>
              )}
            </span>
            {row.status && (
              <span className="companion-status" data-tone={row.tone}>
                {row.status}
              </span>
            )}
            {field.method && row.action && (
              <button
                className="secondary"
                disabled={busy !== null}
                aria-busy={rowBusy ? true : undefined}
                aria-label={`${row.action}: ${row.label}`}
                onClick={() => onRow(field.id, row.id)}
              >
                {rowBusy ? "Working…" : row.action}
              </button>
            )}
          </li>
        );
      })}
    </ul>
  );
}

/** A field with nothing to show is left out rather than drawn as a dash. */
const hasValue = (value: CompanionValue | undefined) =>
  value !== null && value !== undefined && value !== "";

/** The drawing of a companion view for given values; it holds no state. */
export function CompanionPanel({
  view,
  state,
  onAction,
  onRow = () => {},
}: {
  view: CompanionViewDescriptor;
  state: CompanionViewState;
  onAction(actionId: string): void;
  onRow?(fieldId: string, rowId: string): void;
}) {
  return (
    <div className="companion-view" aria-busy={state.busy ? true : undefined}>
      {view.fields
        .filter(
          (field) =>
            hasValue(state.values[field.id]) &&
            (field.type !== "list" || listRows(state.values[field.id]).length),
        )
        .map((field) => (
          <div
            className="companion-field"
            key={field.id}
            data-type={field.type}
          >
            <span className="companion-label">{field.label}</span>
            {field.type === "list" ? (
              <ListField
                field={field}
                value={state.values[field.id]}
                busy={state.busy}
                onRow={onRow}
              />
            ) : (
              <FieldValue
                type={field.type}
                label={field.label}
                value={state.values[field.id]}
              />
            )}
          </div>
        ))}
      {view.actions.length > 0 && (
        <div className="companion-actions">
          {view.actions.map((action, index) => (
            <button
              key={action.id}
              // The first action is the view's main one.
              className={index === 0 ? "primary" : "secondary"}
              disabled={state.busy !== null}
              aria-busy={state.busy === action.id ? true : undefined}
              onClick={() => onAction(action.id)}
            >
              {state.busy === action.id ? "Working…" : action.label}
            </button>
          ))}
        </div>
      )}
      {state.message && (
        <p className="companion-message" role="status">
          {state.message}
        </p>
      )}
      {state.error && (
        <p className="settings-error" role="alert">
          {state.error}
        </p>
      )}
    </div>
  );
}

/** A view whose values and button effects come from a companion process. */
export function CompanionView({
  extensionId,
  surfaceId,
  view,
}: {
  extensionId: string;
  surfaceId: string;
  view: CompanionViewDescriptor;
}) {
  const [state, setState] = useState(INITIAL_COMPANION_STATE);
  const controller = useMemo(
    () =>
      window.bridge
        ? createCompanionController(
            window.bridge,
            extensionId,
            surfaceId,
            setState,
          )
        : null,
    [extensionId, surfaceId],
  );
  useEffect(() => {
    controller?.start();
    return () => controller?.stop();
  }, [controller]);
  if (!controller)
    return <p className="companion-message">Not available outside the app.</p>;
  return (
    <CompanionPanel
      view={view}
      state={state}
      onAction={(actionId) => void controller.run(actionId)}
      onRow={(fieldId, rowId) => void controller.runRow(fieldId, rowId)}
    />
  );
}
