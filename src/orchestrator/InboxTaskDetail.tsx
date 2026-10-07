import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ArrowUpRight, GitBranch } from "lucide-react";
import type { AttentionItem } from "../extensions/modules.ts";
import { ownsKey } from "../lib/ownsKey.ts";
import { groupKey } from "../lib/hostGroup.ts";
import { orchestratorClientFor } from "./client.ts";
import { hostOf } from "./hosts.ts";
import {
  criteriaMet,
  errorText,
  implementAttemptCount,
  latestImplementAttempt,
  stageTrack,
} from "./helpers.ts";
import {
  diffFacts,
  inboxEnterAnswer,
  openTask,
  setPick,
  usePicks,
} from "./moduleAttention.ts";
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

/** A text block that scrolls past a height cap, with a fade while there is
 * more below, so a long body never pushes the footer off screen. */
function ClampedText({ text, className }: { text: string; className: string }) {
  const ref = useRef<HTMLParagraphElement>(null);
  const [more, setMore] = useState(false);
  const measure = () => {
    const el = ref.current;
    if (el) setMore(el.scrollTop + el.clientHeight < el.scrollHeight - 1);
  };
  useLayoutEffect(measure, [text]);
  return (
    <p
      ref={ref}
      className={`${className} inbox-clamp${more ? " more" : ""}`}
      onScroll={measure}
    >
      {text}
    </p>
  );
}

/** The Inbox detail of one task row: answer choices, decide, land, diff and
 * cost, with the task keys (1-9, Enter, L, R, E). Drawn by the module's
 * `AttentionDetail`, only for the selected row. */
export function InboxTaskDetail({ item }: { item: AttentionItem }) {
  const tasks = useTasks(true);
  const id = item.key.slice(item.key.indexOf(":") + 1);
  const task = tasks.find((t) => t.id === id && groupKey(t.host) === item.host);
  const picks = usePicks();
  const [notes, setNotes] = useState<Record<string, string>>({}),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
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
    setPick(item.key, option);
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
      setPick(item.key, undefined);
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
  const context =
    item.kind === "answer" ? (task.question?.text ?? "") : (item.meta ?? "");

  const choices = (task.question?.options ?? []).map((option) => (
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

  const met = criteriaMet(task);
  return (
    <>
      <div className="inbox-preview-head">
        <h2 title={item.title}>{item.title}</h2>
        <button
          className="inbox-link"
          onClick={() => openTask(ownerTarget(task))}
        >
          Open task
          <ArrowUpRight size={12} />
        </button>
      </div>
      {context && (
        <ClampedText
          text={context}
          className={
            item.kind === "answer"
              ? "inbox-prompt-question"
              : "inbox-session-line"
          }
        />
      )}
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
        <div className="inbox-foot">
          <div className="inbox-prompt-options">{choices}</div>
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
        </div>
      )}
    </>
  );
}
