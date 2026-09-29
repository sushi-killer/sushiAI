import { useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import { orchestratorClient } from "./client";
import {
  ageLabel,
  formatCost,
  implementAttemptCount,
  repoName,
  STAGES,
  type StageStep,
  taskReason,
  taskTone,
} from "./helpers";
import { ownerTasks } from "./ownerAttention";
import type { Task } from "./types";
import { AttentionItem, Chip, StageTrack } from "./ui";

const TRY = [
  "Fix the failing test in src/app",
  "Add a setting for the tray icon",
  "Review my branch before I merge",
];

/** The same track every task walks, all pending, for the first-run page. */
const EMPTY_TRACK: StageStep[] = STAGES.map((stage) => ({
  stage,
  state: "pending",
}));

function FirstRun({ onTry }: { onTry: (text: string) => void }) {
  return (
    <div className="orch-first-run">
      <img src="./sushi.svg" width={44} height={44} alt="" />
      <h2>Tell the orchestrator what should happen</h2>
      <p>
        It plans the work, runs an agent, checks the result against your
        criteria,
        <br />
        asks you only when it has to, and lands the change on your branch.
      </p>
      <div className="orch-first-run-track">
        <StageTrack steps={EMPTY_TRACK} />
      </div>
      <div className="orch-first-run-try">
        <span className="orch-eyebrow">TRY</span>
        <div className="orch-first-run-chips">
          {TRY.map((text) => (
            <Chip key={text} onClick={() => onTry(text)}>
              {text}
            </Chip>
          ))}
        </div>
      </div>
    </div>
  );
}

/** Home: what needs the owner right now. A minimal first cut - the
 * greeting and the NEEDS YOU list; lane L2 completes the Figma frame. */
export function HomeView({
  cwd,
  tasks,
  maxAttempts,
  busy,
  act,
  onOpen,
  onTry,
}: {
  cwd: string;
  tasks: Task[];
  maxAttempts?: number;
  busy: boolean;
  act: (action: () => Promise<Task>) => void;
  onOpen: (id: string) => void;
  onTry: (text: string) => void;
}) {
  const [today, setToday] = useState<number | null>(null);
  const refresh = tasks.map((t) => `${t.id}:${t.updatedAt}`).join();
  useEffect(() => {
    let cancelled = false;
    orchestratorClient
      .costsSummary({ repo: cwd, sinceDays: 1, groupBy: ["stage"] })
      .then((summary) => !cancelled && setToday(summary.totals.costUsd))
      .catch(() => !cancelled && setToday(null));
    return () => {
      cancelled = true;
    };
  }, [cwd, refresh]);

  const live = tasks.filter((t) => !t.archived);
  if (live.length === 0) return <FirstRun onTry={onTry} />;
  const owner = ownerTasks(live);
  const active = live.filter(
    (t) =>
      t.status === "running" ||
      t.status === "drafting" ||
      t.status === "queued" ||
      t.status === "waiting",
  ).length;
  const sub = [
    `${active} task${active === 1 ? "" : "s"} active`,
    today === null ? "" : `${formatCost(today)} today`,
    "orchd on Local",
  ].filter(Boolean);

  return (
    <div className="orch-view-scroll orch-home">
      <div className="orch-greet">
        <h2>
          {owner.length === 0
            ? "Nothing needs you"
            : `${owner.length} thing${owner.length === 1 ? "" : "s"} need${owner.length === 1 ? "s" : ""} you`}
        </h2>
        <p>{sub.join(" · ")}</p>
      </div>
      {owner.length > 0 && (
        <>
          <span className="orch-eyebrow">NEEDS YOU · {owner.length}</span>
          {owner.map((task) => {
            const repo = repoName(task.repo);
            if (task.status === "waiting")
              return (
                <AttentionItem
                  key={task.id}
                  tone="warning"
                  title={task.title}
                  time={ageLabel(task.updatedAt)}
                  question
                  context={task.question?.text}
                  actions={task.question?.options.map((option) => (
                    <Chip
                      key={option}
                      disabled={busy}
                      onClick={() =>
                        act(() =>
                          orchestratorClient.taskAnswer(task.id, option),
                        )
                      }
                    >
                      {option}
                    </Chip>
                  ))}
                  meta={`${repo} · attempt ${implementAttemptCount(task)}/${maxAttempts ?? implementAttemptCount(task)}`}
                  onOpen={() => onOpen(task.id)}
                />
              );
            const failed =
              task.status === "failed" || task.status === "stopped";
            return (
              <AttentionItem
                key={task.id}
                tone={taskTone(task)}
                title={task.title}
                time={ageLabel(task.updatedAt)}
                context={`${taskReason(task, tasks, maxAttempts)} · ${formatCost(task.costUsd)}`}
                actions={
                  failed && (
                    <>
                      <button
                        type="button"
                        className="ui-button secondary"
                        disabled={busy}
                        onClick={() =>
                          act(() => orchestratorClient.taskStart(task.id))
                        }
                      >
                        <RefreshCw size={14} /> Run again
                      </button>
                      <button
                        type="button"
                        className="ui-button ghost"
                        disabled={busy}
                        onClick={() =>
                          act(() => orchestratorClient.taskArchive(task.id))
                        }
                      >
                        Archive
                      </button>
                    </>
                  )
                }
                meta={`${repo} · orchd`}
                onOpen={() => onOpen(task.id)}
              />
            );
          })}
        </>
      )}
    </div>
  );
}
