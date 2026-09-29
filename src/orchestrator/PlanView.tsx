import { useState } from "react";
import { Play, Sparkles } from "lucide-react";
import { orchestratorClient } from "./client";
import { errorText } from "./helpers";
import {
  autopilotOf,
  draftMeta,
  planModel,
  readyDrafts,
  type PlanItem,
} from "./planModel";
import type { Settings, Task } from "./types";
import { Tag } from "./ui";
import "./plan.css";

function StartButton({
  label,
  disabled,
  onClick,
}: {
  label: string;
  disabled?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className="ui-button secondary"
      disabled={disabled}
      onClick={onClick}
    >
      <Play size={14} />
      {label}
    </button>
  );
}

function Dot({ on }: { on: boolean }) {
  return <span className={`plan-dot${on ? " ready" : ""}`} aria-hidden />;
}

function DraftGroup({
  item,
  disabled,
  onStart,
  onOpen,
}: {
  item: PlanItem;
  disabled: boolean;
  onStart: (task: Task) => void;
  onOpen: (id: string) => void;
}) {
  const { task, children } = item;
  const sub =
    task.source === "brainstorm"
      ? `from a brainstorm · split into ${children.length}`
      : `split into ${children.length}`;
  return (
    <div className="plan-card plan-group">
      <div className="plan-row">
        <Dot on={item.ready} />
        <div className="plan-body">
          <button
            type="button"
            className="plan-title"
            onClick={() => onOpen(task.id)}
          >
            {task.title}
          </button>
          <span className="plan-sub">{sub}</span>
        </div>
        <span className="plan-meta">{draftMeta(task)}</span>
        <StartButton
          label="Start"
          disabled={disabled || !item.ready}
          onClick={() => onStart(task)}
        />
      </div>
      <div className="plan-children">
        {children.map((child) => (
          <div key={child.task.id} className="plan-child">
            <Dot on={!child.waits} />
            <div className="plan-body">
              <button
                type="button"
                className="plan-child-title"
                onClick={() => onOpen(child.task.id)}
              >
                {child.task.title}
              </button>
              {child.after.length > 0 && (
                <span className="plan-after">
                  <Tag tone="neutral">after {child.after.join(", ")}</Tag>
                </span>
              )}
            </div>
            <span className="plan-meta">{draftMeta(child.task)}</span>
            {child.waits && <span className="plan-meta">waits</span>}
          </div>
        ))}
      </div>
    </div>
  );
}

function DraftRow({
  item,
  later,
  disabled,
  onStart,
  onOpen,
}: {
  item: PlanItem;
  later: boolean;
  disabled: boolean;
  onStart: (task: Task) => void;
  onOpen: (id: string) => void;
}) {
  const { task } = item;
  const goal = task.goal.split("\n")[0]?.trim();
  return (
    <div className={`plan-card plan-single${later ? " later" : ""}`}>
      <Dot on={!later && item.ready} />
      <div className="plan-body">
        <button
          type="button"
          className="plan-title"
          onClick={() => onOpen(task.id)}
        >
          {task.title}
        </button>
        {!later && goal && goal !== task.title && (
          <span className="plan-sub">{goal}</span>
        )}
      </div>
      <span className="plan-meta">{draftMeta(task)}</span>
      {!later && (
        <StartButton
          label="Start"
          disabled={disabled || !item.ready}
          onClick={() => onStart(task)}
        />
      )}
    </div>
  );
}

/** The Plan view: drafts waiting to run. NEXT/LATER and Autopilot follow the
 * daemon's `bucket`/`order` fields and `autopilot` setting when they exist;
 * until then everything is NEXT, oldest first, and Autopilot shows disabled.
 * `tasks`, `settings`, `busy`, `act` and `onOpen` are the panel's
 * wiring. */
export function PlanView({
  tasks = [],
  settings = null,
  busy = false,
  act,
  onSettings,
  onOpen = () => undefined,
  onBrainstorm,
}: {
  tasks?: Task[];
  settings?: Settings | null;
  busy?: boolean;
  act?: (action: () => Promise<Task>) => void;
  /** The panel's own settings write, so its copy refreshes too. */
  onSettings?: (patch: Partial<Settings>) => Promise<void>;
  onOpen?: (id: string) => void;
  onBrainstorm: () => void;
}) {
  const [autopilotError, setAutopilotError] = useState("");
  const model = planModel(tasks);
  const ready = readyDrafts(model);
  const autopilot = autopilotOf(settings);
  const disabled = busy || !act;
  const nextUp = ready[0]?.title;

  function start(task: Task) {
    act?.(() => orchestratorClient.taskStart(task.id));
  }
  function startReady() {
    act?.(async () => {
      let last: Task | undefined;
      for (const task of ready)
        last = await orchestratorClient.taskStart(task.id);
      return last as Task;
    });
  }
  function toggleAutopilot() {
    if (!settings || autopilot === null || !onSettings) return;
    const value = !autopilot;
    setAutopilotError("");
    const current = (settings as { autopilot?: unknown }).autopilot;
    const next =
      current && typeof current === "object"
        ? { ...current, enabled: value }
        : value;
    onSettings?.({ autopilot: next } as Partial<Settings>).catch((e) =>
      setAutopilotError(errorText(e)),
    );
  }

  return (
    <div className="orch-view-scroll plan-view">
      <div className="plan-head">
        <div className="plan-head-text">
          <h2 className="orch-view-title">Plan</h2>
          <p className="plan-lede">Drafts waiting to run.</p>
        </div>
        <button
          type="button"
          className="ui-button ghost"
          onClick={onBrainstorm}
        >
          <Sparkles size={14} />
          Brainstorm
        </button>
        <StartButton
          label={`Start ${ready.length} ready`}
          disabled={disabled || ready.length === 0}
          onClick={startReady}
        />
      </div>

      <div className="plan-card plan-autopilot">
        <button
          type="button"
          role="switch"
          aria-label="Autopilot"
          aria-checked={autopilot === true}
          className={`plan-toggle${autopilot ? " on" : ""}`}
          disabled={autopilot === null}
          onClick={toggleAutopilot}
        >
          <span className="plan-toggle-knob" />
        </button>
        <div className="plan-autopilot-text">
          <span className="plan-title static">Autopilot</span>
          <span className="plan-desc">
            Runs ready drafts one after another in the background, respecting
            dependencies. Stops only for your questions.
          </span>
        </div>
        {autopilot === null ? (
          <span className="plan-meta">coming soon</span>
        ) : (
          autopilot &&
          nextUp && <span className="plan-meta">next: {nextUp}</span>
        )}
      </div>
      {autopilotError && (
        <p className="plan-error" role="alert">
          {autopilotError}
        </p>
      )}

      {model.next.length + model.later.length === 0 ? (
        <p className="plan-empty">
          No drafts waiting. Describe a task below, or start a Brainstorm.
        </p>
      ) : (
        <>
          <span className="plan-label">NEXT · {model.next.length}</span>
          {model.next.map((item) =>
            item.children.length > 0 ? (
              <DraftGroup
                key={item.task.id}
                item={item}
                disabled={disabled}
                onStart={start}
                onOpen={onOpen}
              />
            ) : (
              <DraftRow
                key={item.task.id}
                item={item}
                later={false}
                disabled={disabled}
                onStart={start}
                onOpen={onOpen}
              />
            ),
          )}
          {model.later.length > 0 && (
            <>
              <span className="plan-label">LATER · {model.later.length}</span>
              {model.later.map((item) => (
                <DraftRow
                  key={item.task.id}
                  item={item}
                  later
                  disabled={disabled}
                  onStart={start}
                  onOpen={onOpen}
                />
              ))}
            </>
          )}
        </>
      )}
    </div>
  );
}
