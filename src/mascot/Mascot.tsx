import { useEffect, useRef, useState } from "react";
import {
  ArrowRight,
  ChevronLeft,
  ChevronRight,
  Check,
  RefreshCw,
  X,
} from "lucide-react";
import { Character } from "./Character";
import { isTyping, mascotMood, type Typing } from "./mood";
import type { MascotNotice, TaskMascotNotice } from "./types";
import { doneMeta, noticeHeader } from "../orchestrator/notices";
import {
  clickAnswer,
  enterAnswer,
  shownPick,
  togglePick,
  type AnswerChoice,
} from "../orchestrator/ownerAttention";

const FADE_MS = 700;

const cleanError = (failure: unknown) =>
  String(failure instanceof Error ? failure.message : failure).replace(
    /^Error invoking remote method '[^']*': (Error: )?/,
    "",
  );

/** A question notice's bold line (its first sentence) and the rest. */
function splitQuestion(text: string): { lead: string; rest: string } {
  const trimmed = text.trim();
  const match = /^(.+?[.?!])\s+([\s\S]+)$/.exec(trimmed);
  const [lead, rest] = match
    ? [match[1], match[2]]
    : [trimmed.split("\n")[0], trimmed.split("\n").slice(1).join("\n")];
  return { lead, rest: rest.trim() };
}

function Kbd({ children }: { children: string }) {
  return <kbd className="kbd">{children}</kbd>;
}

function Head({
  label,
  source,
  tone,
  onDismiss,
}: {
  label: string;
  source: string;
  tone: string;
  onDismiss: () => void;
}) {
  return (
    <div className="bubble-head">
      <span className={`tag ${tone}`}>
        <i />
        {label}
      </span>
      {source && <span className="bubble-source">{source}</span>}
      <button
        className="bubble-dismiss"
        aria-label="Dismiss"
        onClick={onDismiss}
      >
        <X size={12} />
      </button>
    </div>
  );
}

const TONES: Record<string, string> = {
  input: "warn",
  done: "ok",
  landing: "info",
  failed: "bad",
  stopped: "bad",
  "core-update": "info",
};

function Bubble({
  notice,
  onTyping,
}: {
  notice: MascotNotice;
  onTyping: (typing: Typing) => void;
}) {
  const [text, setText] = useState("");
  // The owner's own pick: undefined until they pick, "" once they unpick.
  // The first option only shows picked (Figma "Needs you") - a click on send
  // takes it, Enter never does.
  const [pick, setPick] = useState<string | undefined>(undefined);
  const [shownAt] = useState(() => Date.now());
  const [error, setError] = useState("");
  const [fading, setFading] = useState(false);
  const [focused, setFocused] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(timer);
  }, []);
  const bridge = window.mascot;
  const options = notice.kind === "input" ? (notice.options ?? []) : [];

  useEffect(() => {
    onTyping({ id: notice.id, on: focused || text.length > 0 });
  }, [focused, text, notice.id, onTyping]);
  useEffect(
    () => () => onTyping({ id: notice.id, on: false }),
    [notice.id, onTyping],
  );

  useEffect(() => {
    setFading(false);
    if (notice.expiresAt === null) return;
    const timer = setTimeout(
      () => setFading(true),
      Math.max(0, notice.expiresAt - Date.now() - FADE_MS),
    );
    return () => clearTimeout(timer);
  }, [notice.id, notice.expiresAt]);

  const answer = async (taskId: string, value: string) => {
    setError("");
    try {
      await bridge?.answer(taskId, value);
      setText("");
    } catch (failure) {
      setError(cleanError(failure));
    }
  };
  const choice: AnswerChoice = {
    pick,
    preselected: options[0] ?? "",
    note: text,
  };
  const value = clickAnswer(choice);
  const send = (answerText: string) => {
    if (notice.kind === "input" && answerText)
      void answer(notice.taskId, answerText);
  };
  const sendByKey = () => send(enterAnswer(choice, shownAt, Date.now()));
  const keys = useRef({ sendByKey, options });
  keys.current = { sendByKey, options };

  // 1-N picks an option and Enter sends what was picked or typed, unless the
  // reply field has focus (digits are text there, and its form sends on
  // Enter itself).
  const open = notice.kind === "input" && !notice.answered;
  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target as HTMLElement | null;
      if (target?.tagName === "INPUT" || target?.tagName === "TEXTAREA") return;
      const digit = Number(event.key);
      if (digit >= 1 && digit <= options.length) {
        event.preventDefault();
        setPick(keys.current.options[digit - 1]);
      } else if (event.key === "Enter" && target?.tagName !== "BUTTON") {
        event.preventDefault();
        keys.current.sendByKey();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, options.length]);

  const dismiss = () => void bridge?.dismiss(notice.id);

  if (notice.kind === "core-update")
    return (
      <div
        className={`bubble core-update ${fading ? "leaving" : ""}`}
        role="status"
      >
        <Head
          label="Update"
          source=""
          tone={TONES["core-update"]}
          onDismiss={dismiss}
        />
        <strong title={notice.title}>{notice.title}</strong>
        <span className="bubble-body clamped" title={notice.body}>
          {notice.body}
        </span>
        <div className="bubble-actions">
          <button
            className="bubble-primary"
            onClick={() => void bridge?.restart()}
          >
            Restart
          </button>
        </div>
      </div>
    );
  if (notice.answered)
    return (
      <div className="bubble answered" role="status">
        <div className="bubble-head">
          <span className="tag ok">
            <i />
            Answered
          </span>
          <span className="bubble-source">
            {noticeHeader(notice, now).source}
          </span>
        </div>
        <div className="bubble-msg">
          <Check size={14} />
          <span>Answered — the task carries on.</span>
        </div>
      </div>
    );

  const header = noticeHeader(notice, now);
  const openTask = () => void bridge?.open(notice.taskId, notice.focus);
  const meta = notice.kind === "done" ? doneMeta(notice) : null;
  const land = () =>
    void bridge
      ?.land(notice.taskId)
      .catch((failure) => setError(cleanError(failure)));
  const rerun = () =>
    void bridge
      ?.rerun(notice.taskId)
      .catch((failure) => setError(cleanError(failure)));
  return (
    <div
      className={`bubble ${notice.kind} ${fading ? "leaving" : ""}`}
      role="status"
    >
      <Head
        label={header.label}
        source={header.source}
        tone={TONES[notice.kind] ?? "info"}
        onDismiss={dismiss}
      />
      {notice.kind === "input" ? (
        <QuestionLines notice={notice} />
      ) : (
        <strong title={notice.title}>{notice.title}</strong>
      )}
      {notice.kind === "input" ? (
        <div className="bubble-scroll">
          {splitQuestion(notice.body).rest && (
            <span className="bubble-body">
              {splitQuestion(notice.body).rest}
            </span>
          )}
          {!!options.length && (
            <div className="bubble-options">
              {options.map((option) => (
                <button
                  key={option}
                  className={shownPick(choice) === option ? "picked" : ""}
                  aria-pressed={shownPick(choice) === option}
                  onClick={() => {
                    setPick(togglePick(choice, option));
                    setText("");
                  }}
                >
                  {option}
                </button>
              ))}
            </div>
          )}
        </div>
      ) : (
        <span className="bubble-body clamped" title={meta ?? notice.body}>
          {meta ?? notice.body}
        </span>
      )}
      {notice.kind === "input" && (
        <>
          <form
            className="bubble-answer"
            onSubmit={(event) => {
              // Enter in the field: only typed text or an explicit pick.
              event.preventDefault();
              sendByKey();
            }}
          >
            <input
              value={text}
              maxLength={2000}
              placeholder="Type an answer..."
              aria-label="Answer"
              onChange={(event) => setText(event.target.value)}
              onFocus={() => setFocused(true)}
              onBlur={() => setFocused(false)}
            />
            <button
              type="button"
              className="kbd-button"
              aria-label="Send answer"
              disabled={!value}
              onClick={() => send(value)}
            >
              <Kbd>⏎</Kbd>
            </button>
          </form>
          <div className="bubble-hint">
            {!!options.length && (
              <>
                <Kbd>{options.length > 1 ? `1-${options.length}` : "1"}</Kbd>
                <span>pick</span>
              </>
            )}
            <Kbd>⏎</Kbd>
            <span>send</span>
            <button className="bubble-link" onClick={openTask}>
              Open task
            </button>
          </div>
          {error && <span className="bubble-error">{error}</span>}
        </>
      )}
      {notice.kind === "done" && (
        <div className="bubble-actions">
          {notice.canLand && (
            <button className="bubble-primary" onClick={land}>
              <Check size={14} />
              Land
            </button>
          )}
          <button onClick={openTask}>View diff</button>
        </div>
      )}
      {(notice.kind === "failed" || notice.kind === "stopped") && (
        <div className="bubble-actions">
          <button onClick={rerun}>
            <RefreshCw size={14} />
            Run again
          </button>
          <button className="bubble-ghost" onClick={openTask}>
            Open
          </button>
        </div>
      )}
      {notice.kind === "landing" && (
        <div className="bubble-actions">
          <button onClick={openTask}>Open</button>
        </div>
      )}
      {notice.kind !== "input" && error && (
        <span className="bubble-error">{error}</span>
      )}
    </div>
  );
}

/** A question bubble's head: the question itself in bold, then the task and
 * who asked it (Figma "Needs you"). */
function QuestionLines({ notice }: { notice: TaskMascotNotice }) {
  const { lead } = splitQuestion(notice.body);
  const by = notice.askedBy;
  return (
    <>
      <strong title={notice.body}>{lead}</strong>
      <span className="bubble-sub" title={notice.title}>
        {by ? `${notice.title} · asked by ${by}` : notice.title}
      </span>
    </>
  );
}

/** The desktop mascot: the sushi with a speech bubble for the newest queued
 * orchd notice, a pager ("2 of 4") for the rest, and a pill it collapses to. */
export function Mascot() {
  const [notices, setNotices] = useState<MascotNotice[]>([]);
  const [index, setIndex] = useState(0);
  const [hops, setHops] = useState(0);
  const [collapsed, setCollapsed] = useState(false);
  const [typing, setTyping] = useState<Typing>({ id: "", on: false });
  const [focusAnswer, setFocusAnswer] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const lastHeight = useRef(0);
  const presenting = useRef(false);

  useEffect(
    () =>
      window.mascot?.onNotices((next) => {
        setNotices((previous) => {
          if (next[0]?.id !== previous[0]?.id) {
            setIndex(0);
            setHops((count) => count + 1);
            // A fullscreen app or a slideshow keeps new notices in the pill.
            if (!presenting.current) setCollapsed(false);
          }
          return next;
        });
      }),
    [],
  );

  // Collapses when presenting starts and expands when it ends; in between,
  // clicking the sushi or pressing Option-Space still toggles by hand.
  useEffect(
    () =>
      window.mascot?.onPresenting((next) => {
        if (next === presenting.current) return;
        presenting.current = next;
        setCollapsed(next);
      }),
    [],
  );

  useEffect(
    () =>
      window.mascot?.onToggle(() =>
        setCollapsed((value) => {
          if (value) setFocusAnswer((count) => count + 1);
          return !value;
        }),
      ),
    [],
  );

  // Option-Space expanded the stack: the answer field takes typing at once.
  useEffect(() => {
    if (!focusAnswer || collapsed) return;
    void window.mascot
      ?.focus()
      .then(() =>
        rootRef.current
          ?.querySelector<HTMLInputElement>(".bubble-answer input")
          ?.focus(),
      );
  }, [focusAnswer, collapsed]);

  const shown = notices[Math.min(index, notices.length - 1)];
  const hasShown = Boolean(shown);

  // Reports the content's natural height (never the window's own): the
  // laid-out children plus whatever the scroll area hides, so a capped window
  // reports the same value it would uncapped.
  useEffect(() => {
    const root = rootRef.current;
    if (!root || !hasShown) return;
    const report = () => {
      const style = getComputedStyle(root);
      const kids = Array.from(root.children) as HTMLElement[];
      const gap = parseFloat(style.rowGap) || 0;
      const scroll = root.querySelector<HTMLElement>(".bubble-scroll");
      const hidden = scroll
        ? Math.max(0, scroll.scrollHeight - scroll.clientHeight)
        : 0;
      const height = Math.ceil(
        (parseFloat(style.paddingTop) || 0) +
          (parseFloat(style.paddingBottom) || 0) +
          kids.reduce((sum, kid) => sum + kid.offsetHeight, 0) +
          gap * Math.max(0, kids.length - 1) +
          hidden,
      );
      if (height === lastHeight.current) return;
      lastHeight.current = height;
      window.mascot?.resize(height);
    };
    const observer = new ResizeObserver(report);
    observer.observe(root);
    root
      .querySelectorAll(".bubble-wrap, .bubble-scroll, .bubble-wrap > *")
      .forEach((node) => observer.observe(node));
    report();
    return () => observer.disconnect();
  });

  if (!shown) return null;
  const total = notices.length;
  const step = (delta: number) =>
    setIndex(
      (Math.min(index, notices.length - 1) + delta + notices.length) %
        notices.length,
    );
  const mood = mascotMood(shown, isTyping(typing, shown));
  const badge = total > 1 ? String(total) : mood === "done" ? "\u2713" : null;
  const needs = notices.filter((item) => item.kind === "input").length;
  const character = (
    <button
      className={`sushi-hop hop-${hops % 2}`}
      aria-label={collapsed ? "Expand notices" : "Collapse to a pill"}
      aria-expanded={!collapsed}
      onClick={() => setCollapsed((value) => !value)}
    >
      <Character mood={mood} />
      {badge && (
        <span className={`sushi-badge ${mood === "done" ? "ok" : ""}`}>
          {badge}
        </span>
      )}
    </button>
  );
  if (collapsed)
    return (
      <div className="mascot" ref={rootRef}>
        <div className="pill" role="status">
          {character}
          <div className="pill-text">
            <span className="pill-title">
              {needs
                ? `${needs} need${needs === 1 ? "s" : ""} you`
                : `${total} notice${total === 1 ? "" : "s"}`}
            </span>
            <span className="pill-sub">
              {shown.title}
              {total > 1 ? ` · +${total - 1}` : ""}
            </span>
          </div>
          <Kbd>{"\u2325 Space"}</Kbd>
        </div>
      </div>
    );
  return (
    <div className="mascot" ref={rootRef}>
      <div className="bubble-wrap">
        {total > 1 && (
          <div className="queue">
            <button aria-label="Previous notice" onClick={() => step(-1)}>
              <ChevronLeft size={13} />
            </button>
            <span>
              {Math.min(index, total - 1) + 1} of {total}
            </span>
            <button aria-label="Next notice" onClick={() => step(1)}>
              <ChevronRight size={13} />
            </button>
            <button
              className="queue-inbox"
              onClick={() => void window.mascot?.openInbox()}
            >
              Answer all in Inbox
              <ArrowRight size={12} />
            </button>
          </div>
        )}
        <Bubble
          key={`${shown.id}:${shown.kind !== "core-update" && shown.answered}`}
          notice={shown}
          onTyping={setTyping}
        />
      </div>
      {character}
    </div>
  );
}
