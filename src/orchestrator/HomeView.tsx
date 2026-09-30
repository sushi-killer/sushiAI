import { useEffect, useState, type ReactNode } from "react";
import { ArrowRight, ChartColumn, RefreshCw } from "lucide-react";
import { Character } from "../mascot/Character";
import { useOrchestratorClient } from "./hostContext";
import {
  errorText,
  formatCost,
  railGroups,
  STAGES,
  type StageStep,
  taskReason,
  taskTone,
} from "./helpers";
import {
  activeCount,
  clearLine,
  greeting,
  questionMeta,
  runNoteMessage,
  runningMeta,
  sameFailureNote,
  subLine,
} from "./homeModel";
import { elapsedLabel, ownerTasks, projectName } from "./ownerAttention";
import { weekSummary } from "./stats";
import type { SpendSummary, Task } from "./types";
import { AttentionItem, Chip, StageTrack } from "./ui";
import "./home.css";

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
    <div className="home-first-run">
      <img src="./sushi.svg" width={44} height={44} alt="" />
      <h2>Tell the orchestrator what should happen</h2>
      <p>
        It plans the work, runs an agent, checks the result against your
        criteria,
        <br />
        asks you only when it has to, and lands the change on your branch.
      </p>
      <div className="home-first-run-track">
        <StageTrack steps={EMPTY_TRACK} />
      </div>
      <div className="home-first-run-try">
        <span className="orch-eyebrow">TRY</span>
        <div className="home-first-run-chips">
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

function AnalyticsLine({
  tasks,
  spend,
  onOpenAnalytics,
}: {
  tasks: Task[];
  spend: SpendSummary | null;
  onOpenAnalytics?: () => void;
}) {
  const text = weekSummary(tasks, spend).text;
  return (
    <div className="home-analytics">
      <ChartColumn size={13} />
      <span className="home-analytics-text">This week: {text}</span>
      {onOpenAnalytics && (
        <button
          type="button"
          className="home-analytics-link"
          onClick={onOpenAnalytics}
        >
          Analytics <ArrowRight size={12} />
        </button>
      )}
    </div>
  );
}

/** "Run with a note": the note goes to the orchestrator chat as an
 * instruction for this task; the orchestrator agent decides how to apply it
 * (`task.amend` would turn it into a permanent criterion). */
function NoteForm({
  task,
  onSend,
  onCancel,
}: {
  task: Task;
  onSend: (text: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [note, setNote] = useState("");
  const [sending, setSending] = useState(false);
  const [sent, setSent] = useState(false);
  const [error, setError] = useState("");
  const submit = () => {
    const text = note.trim();
    if (!text || sending) return;
    setSending(true);
    setError("");
    onSend(runNoteMessage(task, text))
      .then(() => setSent(true))
      .catch((e) => setError(errorText(e)))
      .finally(() => setSending(false));
  };
  if (sent)
    return (
      <span className="home-note-sent" role="status">
        Sent to the orchestrator - it will run this task again with your note.
        <button type="button" className="ui-button ghost" onClick={onCancel}>
          Done
        </button>
      </span>
    );
  return (
    <form
      className="home-note"
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      <input
        autoFocus
        value={note}
        placeholder="What should the next run do differently?"
        aria-label="Note for the next run"
        onChange={(event) => setNote(event.target.value)}
        onKeyDown={(event) => event.key === "Escape" && onCancel()}
      />
      <button
        type="submit"
        className="ui-button secondary"
        disabled={sending || !note.trim()}
      >
        Send
      </button>
      <button type="button" className="ui-button ghost" onClick={onCancel}>
        Cancel
      </button>
      {error && (
        <span className="home-note-error" role="alert">
          {error}
        </span>
      )}
    </form>
  );
}

/** Home: what needs the owner right now, what is running (narrow only - the
 * rail shows it otherwise), the week in one line. */
export function HomeView({
  cwd,
  tasks,
  maxAttempts,
  busy,
  act,
  onOpen,
  onOpenAnalytics,
  onSendNote,
  onTry,
  remoteHost,
  hostStrip,
}: {
  cwd: string;
  tasks: Task[];
  maxAttempts?: number;
  busy: boolean;
  act: (action: () => Promise<Task>) => void;
  onOpen: (id: string) => void;
  /** Opens the Analytics view; the link is hidden without it. */
  onOpenAnalytics?: () => void;
  /** Sends a message to the orchestrator chat; rejects when it fails. */
  onSendNote: (text: string) => Promise<void>;
  onTry: (text: string) => void;
  /** The remote host the daemon runs on; unset on this Mac. */
  remoteHost?: string;
  /** Shown under the greeting: the remote host's preflight strip. */
  hostStrip?: ReactNode;
}) {
  const orchestratorClient = useOrchestratorClient();
  const [today, setToday] = useState<number | null>(null);
  const [week, setWeek] = useState<SpendSummary | null>(null);
  const [noteFor, setNoteFor] = useState<string | null>(null);
  const refresh = tasks.map((t) => `${t.id}:${t.updatedAt}`).join();
  useEffect(() => {
    let cancelled = false;
    orchestratorClient
      .costsSummary({ repo: cwd, sinceDays: 1, groupBy: ["stage"] })
      .then((summary) => !cancelled && setToday(summary.totals.costUsd))
      .catch(() => !cancelled && setToday(null));
    orchestratorClient
      .costsSummary({ repo: cwd, sinceDays: 7, groupBy: ["stage"] })
      .then((summary) => !cancelled && setWeek(summary))
      .catch(() => !cancelled && setWeek(null));
    return () => {
      cancelled = true;
    };
  }, [cwd, refresh, orchestratorClient]);

  const live = tasks.filter((t) => !t.archived);
  if (live.length === 0) return <FirstRun onTry={onTry} />;
  const owner = ownerTasks(live);
  const groups = railGroups(live);
  const analytics = (
    <AnalyticsLine
      tasks={live}
      spend={week}
      onOpenAnalytics={onOpenAnalytics}
    />
  );

  if (owner.length === 0)
    return (
      <div className="orch-view-scroll home">
        <div className="home-clear">
          <span className="home-mascot">
            <Character mood="idle" />
          </span>
          <div className="home-greet">
            <h2>{greeting(0)}</h2>
            <p>{clearLine(groups.running.length, groups.landedToday.length)}</p>
          </div>
        </div>
        {hostStrip}
        {analytics}
      </div>
    );

  return (
    <div className="orch-view-scroll home">
      <div className="home-greet home-greeting">
        <h2>{greeting(owner.length)}</h2>
        <p>{subLine(activeCount(live), today, remoteHost)}</p>
      </div>
      {hostStrip}
      <span className="orch-eyebrow">NEEDS YOU · {owner.length}</span>
      {owner.map((task) => {
        const repo = projectName(task.repo);
        if (task.status === "waiting")
          return (
            <AttentionItem
              key={task.id}
              tone="warning"
              title={task.title}
              time={elapsedLabel(Date.now() - task.updatedAt)}
              question
              context={task.question?.text}
              actions={task.question?.options.map((option) => (
                <Chip
                  key={option}
                  disabled={busy}
                  onClick={() =>
                    act(() => orchestratorClient.taskAnswer(task.id, option))
                  }
                >
                  {option}
                </Chip>
              ))}
              meta={questionMeta(repo, task, maxAttempts)}
              onOpen={() => onOpen(task.id)}
            />
          );
        const failed = task.status === "failed" || task.status === "stopped";
        return (
          <AttentionItem
            key={task.id}
            tone={taskTone(task)}
            title={task.title}
            time={elapsedLabel(Date.now() - task.updatedAt)}
            context={`${taskReason(task, tasks, maxAttempts)} \u00b7 ${formatCost(task.costUsd)}${sameFailureNote(task)}`}
            actions={
              failed &&
              (noteFor === task.id ? (
                <NoteForm
                  task={task}
                  onSend={onSendNote}
                  onCancel={() => setNoteFor(null)}
                />
              ) : (
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
                    onClick={() => setNoteFor(task.id)}
                  >
                    Run with a note
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
              ))
            }
            meta={`${repo} \u00b7 orchd`}
            onOpen={() => onOpen(task.id)}
          />
        );
      })}
      {groups.running.length > 0 && (
        <div className="home-running">
          <span className="orch-eyebrow">
            RUNNING · {groups.running.length}
          </span>
          {groups.running.map((task) => (
            <button
              key={task.id}
              type="button"
              className="home-run"
              onClick={() => onOpen(task.id)}
            >
              <span className={`home-run-dot ui-tone-${taskTone(task)}`} />
              <span className="home-run-title">{task.title}</span>
              <span className="home-run-meta">{runningMeta(task)}</span>
            </button>
          ))}
        </div>
      )}
      {analytics}
    </div>
  );
}
