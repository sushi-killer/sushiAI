import type { ReactNode, Ref } from "react";
import { ArrowUp, Sparkles, Square } from "lucide-react";
import { ChipPicker } from "../ChipPicker";
import type { Settings } from "./types";

/** Pane/Composer: the one input at the bottom of the orchestrator - a new
 * task on Home, a message in Chat. The caller owns the draft and decides
 * what a send does; `route` is the picker (or label) left in the bar, and
 * `extra` anything else that belongs there (the base branch). */
export function Composer({
  value,
  onChange,
  onSubmit,
  placeholder,
  ariaLabel,
  sendLabel,
  disabled = false,
  sending = false,
  onStop,
  route,
  extra,
  host = "Local CLI",
  inputRef,
}: {
  value: string;
  onChange: (value: string) => void;
  onSubmit: () => void;
  placeholder: string;
  ariaLabel: string;
  sendLabel: string;
  disabled?: boolean;
  /** A send is in flight: the send button waits (or stops, with `onStop`). */
  sending?: boolean;
  onStop?: () => void;
  route?: ReactNode;
  extra?: ReactNode;
  host?: string;
  inputRef?: Ref<HTMLTextAreaElement>;
}) {
  const canSend = !disabled && !sending && value.trim().length > 0;
  return (
    <div className={`orch-composer${disabled ? " disabled" : ""}`}>
      <textarea
        ref={inputRef}
        rows={1}
        aria-label={ariaLabel}
        placeholder={placeholder}
        value={value}
        disabled={disabled}
        onChange={(event) => {
          onChange(event.target.value);
          const el = event.target;
          el.style.height = "";
          el.style.height = `${Math.min(el.scrollHeight, 220)}px`;
        }}
        onKeyDown={(event) => {
          if (
            event.key === "Enter" &&
            !event.shiftKey &&
            !event.nativeEvent.isComposing
          ) {
            event.preventDefault();
            if (canSend) onSubmit();
          }
        }}
      />
      <div className="orch-composer-bar">
        {route}
        <span className="orch-composer-host">{host}</span>
        {extra}
        <span className="orch-composer-spacer" />
        {sending && onStop ? (
          <button
            type="button"
            className="orch-composer-send enabled"
            aria-label="Stop response"
            onClick={onStop}
          >
            <Square size={12} />
          </button>
        ) : (
          <button
            type="button"
            className={`orch-composer-send${canSend ? " enabled" : ""}`}
            aria-label={sendLabel}
            disabled={!canSend}
            onClick={onSubmit}
          >
            <ArrowUp size={14} />
          </button>
        )}
      </div>
    </div>
  );
}

/** The orchestrator chat's route picker: "" is the standard tier's route. */
export function OrchestratorRouteChip({
  settings,
  onRouteChange,
}: {
  settings: Settings;
  onRouteChange: (routeId: string) => void;
}) {
  const routeId = settings.orchestrator || "";
  const route = settings.routes.find((r) => r.id === routeId);
  return (
    <ChipPicker
      icon={<Sparkles size={14} className="orch-composer-route-icon" />}
      label={routeId ? (route?.label ?? routeId) : "Standard route"}
      ariaLabel="Orchestrator route"
      value={routeId}
      onChange={onRouteChange}
      options={[
        { value: "", label: "Standard route" },
        ...settings.routes.map((r) => ({ value: r.id, label: r.label })),
      ]}
    />
  );
}

/** A new task has no route of its own - its tier picks one - so the task
 * composer names it without offering a menu. */
export function TaskRouteLabel() {
  return (
    <span
      className="chip orch-composer-route-static"
      title="A task's tier picks its route; set tiers in Settings"
    >
      <Sparkles size={14} className="orch-composer-route-icon" />
      <span>Standard route</span>
    </span>
  );
}
