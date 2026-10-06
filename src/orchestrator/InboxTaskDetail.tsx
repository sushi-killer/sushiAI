import { useEffect, useRef, useState } from "react";
import { ArrowUpRight, GitBranch, RefreshCw } from "lucide-react";
import type { AttentionItem } from "../extensions/modules.ts";
import { ownsKey } from "../app/inboxModel.ts";
import { groupKey } from "../app/workspaceMerge.ts";
import { orchestratorClientFor } from "./client.ts";
import { hostOf } from "./hosts.ts";
import {
  criteriaMet,
  errorText,
  formatCost,
  implementAttemptCount,
  latestImplementAttempt,
  reviewOf,
  stageTrack,
  taskReason,
} from "./helpers.ts";
import { diffFacts, inboxEnterAnswer, openTask } from "./moduleAttention.ts";
import {
  clickReply,
  shownPick,
  togglePick,
  type ReplyChoice,
} from "../lib/reply.ts";
import { ownerTarget } from "./ownerAttention.ts";
import { shortBranch } from "./taskDetailModel.ts";
import type { Task } from "./types.ts";
import { useTasks } from "./useTasks.ts";
import { Chip, Criterion, StageTrack } from "../ui/index.ts";

/** orchd's `diffStat`: the task's branch against its base, absent until
 * its first implement attempt. */
type DiffStat = { files: number; added: number; removed: number };
const diffOf = (task: Task): DiffStat | undefined =>
  (task as Task & { diffStat?: DiffStat }).diffStat;

const files = (count: number) => `${count} file${count === 1 ? "" : "s"}`;

/** The Inbox detail of one task row: answer choices, decide, land, diff and
 * cost, with the task keys (1-9, Enter, L, R, E). Drawn by the module's
 * `AttentionDetail`, only for the selected row. */
export function InboxTaskDetail({ item }: { item: AttentionItem }) {
  const tasks = useTasks(true);
  const id = item.key.slice(item.key.indexOf(":") + 1);
  const task = tasks.find((t) => t.id === id && groupKey(t.host) === item.host);
  const [picks, setPicks] = useState<Record<string, string>>({}),
    [notes, setNotes] = useState<Record<string, string>>({}),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [fixing, setFixing] = useState<string | null>(null),
    [fixNote, setFixNote] = useState(""),
    [maxAttempts, setMaxAttempts] = useState<Record<string, number>>({});
  const inFlight = useRef(false);
  const pickedAt = useRef<Record<string, number>>({});
  /** When the selection last moved here: Enter is ignored for a moment
   * after, so a held or doubled Enter never answers the next item. */
  const selectedAt = useRef(0);
  useEffect(() => {
    selectedAt.current = Date.now();
  }, [item.key]);

  const host = task ? hostOf(task) : undefined;
  useEffect(() => {
    if (maxAttempts[item.host] != null || !task) return;
    let live = true;
    orchestratorClientFor(host)
      .settingsGet()
      .then((settings) => {
        if (live)
          setMaxAttempts((old) => ({
            ...old,
            [item.host]: settings.maxAttempts,
          }));
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [item.host, host, maxAttempts, task]);

  const note = notes[item.key] ?? "";
  const choice: ReplyChoice = {
    pick: picks[item.key],
    preselected: task?.question?.options[0] ?? "",
    note,
  };
  const setNote = (text: string) =>
    setNotes((old) => ({ ...old, [item.key]: text }));
  const pickOption = (option: string) => {
    pickedAt.current[item.key] = Date.now();
    setPicks((old) => ({ ...old, [item.key]: option }));
  };
  async function act(run: () => Promise<unknown>) {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError("");
    try {
      await run();
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  }
  const client = () => orchestratorClientFor(host);
  const answer = (text: string) => {
    if (!task || !text) return;
    void act(async () => {
      await client().taskAnswer(task.id, text);
      const drop = (old: Record<string, string>) => {
        const next = { ...old };
        delete next[item.key];
        return next;
      };
      setNotes(drop);
      setPicks(drop);
      delete pickedAt.current[item.key];
    });
  };
  const answerByKey = () =>
    answer(
      inboxEnterAnswer(
        choice,
        selectedAt.current,
        pickedAt.current[item.key],
        Date.now(),
      ),
    );
  const runAgain = () => task && void act(() => client().taskStart(task.id));
  const archive = () => task && void act(() => client().taskArchive(task.id));
  const land = () => task && void act(() => client().taskLand(task.id));

  const handlers = useRef<(event: KeyboardEvent) => void>(() => {});
  handlers.current = (event) => {
    if (!task || busy) return;
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const key = event.key.toLowerCase();
    if (ownsKey(event.target, key)) return;
    if (item.kind === "answer" && /^[1-9]$/.test(key)) {
      const option = task.question?.options[Number(key) - 1];
      if (option) pickOption(option);
    } else if (key === "enter" && item.kind === "answer") answerByKey();
    else if (key === "l" && item.kind === "review") land();
    else if (key === "r" && item.kind === "decide" && task.status !== "landing")
      runAgain();
    else if (key === "e" && item.kind !== "answer") archive();
  };
  useEffect(() => {
    const listener = (event: KeyboardEvent) => handlers.current(event);
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, []);

  if (!task)
    return (
      <>
        <div className="inbox-preview-head">
          <h2 title={item.title}>{item.title}</h2>
        </div>
        <p className="inbox-session-line">Loading the task…</p>
      </>
    );

  const attempts = implementAttemptCount(task);
  const diff = diffOf(task);
  const changed =
    diff?.files ?? latestImplementAttempt(task)?.changedFiles.length;
  const facts = diffFacts(
    changed,
    attempts,
    maxAttempts[item.host],
    task.costUsd,
  );
  let context: string;
  if (item.kind === "answer")
    context = (task.question?.text ?? "").split("\n")[0];
  else if (item.kind === "review") {
    const count = latestImplementAttempt(task)?.changedFiles.length;
    const review = reviewOf(task);
    context = [
      review ? `review ${review.verdict}` : "done",
      count ? files(count) : "",
      formatCost(task.costUsd),
      `not landed · → ${task.baseRef}`,
    ]
      .filter(Boolean)
      .join(" · ");
  } else context = `${taskReason(task, tasks)} · ${formatCost(task.costUsd)}`;

  /** Needed a fix / Clean: a fix takes a note (the follow-up task is made
   * from it), pressing an active mark clears it. */
  function marks(target: Task) {
    const mark = target.leadTouch;
    if (fixing === target.id)
      return (
        <form
          className="inbox-fix"
          onSubmit={(event) => {
            event.preventDefault();
            void act(async () => {
              await client().taskLeadTouch(target.id, true, fixNote.trim());
              setFixing(null);
              setFixNote("");
            });
          }}
        >
          <input
            aria-label="What had to be fixed"
            placeholder="What is missing or wrong? A follow-up task will be created."
            value={fixNote}
            autoFocus
            onChange={(event) => setFixNote(event.target.value)}
          />
          <button type="submit" className="ui-button primary" disabled={busy}>
            Save
          </button>
          <button
            type="button"
            className="ui-button ghost"
            onClick={() => setFixing(null)}
          >
            Cancel
          </button>
        </form>
      );
    return (
      <>
        <button
          className="ui-button ghost"
          disabled={busy}
          aria-pressed={mark?.touched === true}
          onClick={() => {
            if (mark?.touched === true)
              void act(() => client().taskLeadTouch(target.id));
            else {
              setFixNote("");
              setFixing(target.id);
            }
          }}
        >
          Needed a fix
        </button>
        <button
          className="ui-button ghost"
          disabled={busy}
          aria-pressed={mark?.touched === false}
          onClick={() =>
            void act(() =>
              client().taskLeadTouch(
                target.id,
                mark?.touched === false ? undefined : false,
              ),
            )
          }
        >
          Clean
        </button>
      </>
    );
  }

  let actions;
  if (item.kind === "answer")
    actions = (task.question?.options ?? []).map((option) => (
      <Chip
        key={option}
        selected={shownPick(choice) === option}
        onClick={() => {
          pickOption(togglePick(choice, option));
          setNote("");
        }}
      >
        {option}
      </Chip>
    ));
  else if (item.kind === "decide")
    actions = (
      <>
        {task.status !== "landing" && (
          <button
            className="ui-button secondary"
            disabled={busy}
            title="Run again (R)"
            onClick={runAgain}
          >
            <RefreshCw size={14} /> Run again
          </button>
        )}
        <button
          className="ui-button ghost"
          onClick={() => openTask(ownerTarget(task))}
        >
          Run with a note
        </button>
        <button
          className="ui-button ghost"
          disabled={busy}
          title="Archive (E)"
          onClick={archive}
        >
          Archive
        </button>
      </>
    );
  else
    actions = (
      <>
        <button
          className="ui-button primary"
          disabled={busy}
          title="Land (L)"
          onClick={land}
        >
          Land
        </button>
        {marks(task)}
      </>
    );

  const met = criteriaMet(task);
  return (
    <>
      <div className="inbox-preview-head">
        <h2 title={task.title}>{task.title}</h2>
        <button
          className="inbox-link"
          onClick={() => openTask(ownerTarget(task))}
        >
          Open task
          <ArrowUpRight size={12} />
        </button>
      </div>
      {context && (
        <p
          className={
            item.kind === "answer"
              ? "inbox-prompt-question"
              : "inbox-session-line"
          }
        >
          {context}
        </p>
      )}
      <div className="inbox-prompt-options">{actions}</div>
      <StageTrack steps={stageTrack(task)} />
      {task.criteria.length > 0 && (
        <div className="inbox-criteria">
          <span className="inbox-eyebrow">ACCEPTANCE</span>
          {task.criteria.map((text) => (
            <Criterion key={text} state={met ? "met" : "pending"}>
              {text}
            </Criterion>
          ))}
        </div>
      )}
      <div className="inbox-diff">
        <GitBranch size={13} />
        <span className="inbox-diff-branch" title={task.branch}>
          {shortBranch(task.branch)}
        </span>
        {!!diff?.added && <span className="inbox-diff-add">+{diff.added}</span>}
        {!!diff?.removed && (
          <span className="inbox-diff-del">{`−${diff.removed}`}</span>
        )}
        {facts && <span className="inbox-diff-rest">{`· ${facts}`}</span>}
      </div>
      {error && (
        <p className="inbox-error" role="alert">
          {error}
        </p>
      )}
      <div className="inbox-preview-spacer" />
      {item.kind === "answer" && (
        <form
          className="inbox-reply"
          onSubmit={(event) => {
            // Enter in the field: typed text or an explicit pick only.
            event.preventDefault();
            answerByKey();
          }}
        >
          <input
            aria-label="Answer note"
            placeholder="Answer with a note, or pick above…"
            value={note}
            onChange={(event) => setNote(event.target.value)}
          />
          <button
            type="button"
            className="ui-button primary"
            disabled={busy || !clickReply(choice)}
            onClick={() => answer(clickReply(choice))}
          >
            Answer
          </button>
        </form>
      )}
    </>
  );
}
