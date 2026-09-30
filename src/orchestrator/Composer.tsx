import type { ReactNode, Ref } from "react";
import { ArrowUp, Sparkles, Square } from "lucide-react";
import { ChipPicker } from "../ChipPicker";
import { CHAT_MODES } from "./chatModel";
import type { ChatMode, Settings } from "./types";

/** Pane/Composer: the one input at the bottom of the orchestrator - a new
 * task on Home, a message in Chat. The caller owns the draft and decides
 * what a send does; `route` is the picker (or label) left in the bar, and
 * `extra` anything else that belongs there (the base branch). `mode` and
 * `onModeChange` add the Chat / Brainstorm / Plan switch, only for the
 * orchestrator chat. */
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
  mode,
  onModeChange,
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
  mode?: ChatMode;
  onModeChange?: (mode: ChatMode) => void;
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
        {mode && onModeChange && (
          <>
            <ModeSwitch
              mode={mode}
              disabled={disabled}
              onChange={onModeChange}
            />
            <span className="orch-composer-divider" aria-hidden />
          </>
        )}
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

/** Orch/ModeSwitch: what the next message is for - talk, brainstorm or plan. */
function ModeSwitch({
  mode,
  disabled,
  onChange,
}: {
  mode: ChatMode;
  disabled: boolean;
  onChange: (mode: ChatMode) => void;
}) {
  return (
    <div className="orch-modeswitch" role="group" aria-label="Message mode">
      {CHAT_MODES.map((m) => (
        <button
          key={m.value}
          type="button"
          className={mode === m.value ? "selected" : undefined}
          aria-pressed={mode === m.value}
          disabled={disabled}
          onClick={() => onChange(m.value)}
        >
          {m.label}
        </button>
      ))}
    </div>
  );
}

/** The orchestrator chat's route picker: "" is the standard tier's route. */
export function OrchestratorRouteChip({
  settings,
  onRouteChange,
  standardLabel = "Standard route",
}: {
  settings: Settings;
  onRouteChange: (routeId: string) => void;
  /** What the unset route reads as; the Chat view calls it "Orchestrator". */
  standardLabel?: string;
}) {
  const routeId = settings.orchestrator || "";
  const route = settings.routes.find((r) => r.id === routeId);
  return (
    <ChipPicker
      icon={<Sparkles size={14} className="orch-composer-route-icon" />}
      label={routeId ? (route?.label ?? routeId) : standardLabel}
      ariaLabel="Orchestrator route"
      value={routeId}
      onChange={onRouteChange}
      options={[
        { value: "", label: standardLabel },
        ...settings.routes.map((r) => ({ value: r.id, label: r.label })),
      ]}
    />
  );
}

/** A new task has no route of its own - its tier picks one - so the task
 * composer's route menu names that and holds the one per-task choice left:
 * the base branch. */
export function TaskRouteLabel({
  disabled,
  onBaseBranch,
}: {
  disabled?: boolean;
  onBaseBranch: () => void;
}) {
  return (
    <ChipPicker
      icon={<Sparkles size={14} className="orch-composer-route-icon" />}
      label="Standard route"
      ariaLabel="Task route"
      value=""
      disabled={disabled}
      onChange={(value) => value === "base" && onBaseBranch()}
      options={[
        {
          value: "",
          label: "Standard route",
          title: "A task's tier picks its route; set tiers in Settings",
        },
        { value: "base", label: "Base branch…" },
      ]}
    />
  );
}
