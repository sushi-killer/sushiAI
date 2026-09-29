import { useEffect, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, X } from "lucide-react";
import { Character } from "./Character";
import { isTyping, mascotMood, type Typing } from "./mood";
import type { MascotNotice } from "./types";

const FADE_MS = 700;

function Bubble({
  notice,
  onTyping,
}: {
  notice: MascotNotice;
  onTyping: (typing: Typing) => void;
}) {
  const [text, setText] = useState("");
  const [error, setError] = useState("");
  const [fading, setFading] = useState(false);
  const [focused, setFocused] = useState(false);
  const bridge = window.mascot;

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
      setError(
        String(failure instanceof Error ? failure.message : failure).replace(
          /^Error invoking remote method '[^']*': (Error: )?/,
          "",
        ),
      );
    }
  };

  if (notice.kind === "core-update")
    return (
      <div
        className={`bubble core-update ${fading ? "leaving" : ""}`}
        role="status"
      >
        <button
          className="bubble-dismiss"
          aria-label="Dismiss"
          onClick={() => void bridge?.dismiss(notice.id)}
        >
          <X size={13} />
        </button>
        <strong title={notice.title}>{notice.title}</strong>
        <span className="bubble-body clamped" title={notice.body}>
          {notice.body}
        </span>
        <button className="bubble-open" onClick={() => void bridge?.restart()}>
          Restart
        </button>
      </div>
    );
  if (notice.answered)
    return (
      <div className="bubble answered" role="status">
        <strong>Answered</strong>
        <span>Thanks, the task carries on.</span>
      </div>
    );
  return (
    <div
      className={`bubble ${notice.kind} ${fading ? "leaving" : ""}`}
      role="status"
    >
      <button
        className="bubble-dismiss"
        aria-label="Dismiss"
        onClick={() => void bridge?.dismiss(notice.id)}
      >
        <X size={13} />
      </button>
      <strong title={notice.title}>{notice.title}</strong>
      {notice.kind === "input" ? (
        <div className="bubble-scroll">
          <span className="bubble-body">{notice.body}</span>
          {!!notice.options?.length && (
            <div className="bubble-options">
              {notice.options.map((option) => (
                <button
                  key={option}
                  onClick={() => void answer(notice.taskId, option)}
                >
                  {option}
                </button>
              ))}
            </div>
          )}
        </div>
      ) : (
        <span className="bubble-body clamped" title={notice.body}>
          {notice.body}
        </span>
      )}
      {notice.kind === "input" && (
        <>
          <form
            className="bubble-answer"
            onSubmit={(event) => {
              event.preventDefault();
              void answer(notice.taskId, text);
            }}
          >
            <input
              value={text}
              maxLength={2000}
              placeholder="Type an answer"
              aria-label="Answer"
              onChange={(event) => setText(event.target.value)}
              onFocus={() => setFocused(true)}
              onBlur={() => setFocused(false)}
            />
            <button type="submit" disabled={!text.trim()}>
              Answer
            </button>
          </form>
          {error && <span className="bubble-error">{error}</span>}
        </>
      )}
      {notice.kind === "done" && notice.canLand && (
        <button
          className="bubble-open"
          onClick={() =>
            void bridge
              ?.land(notice.taskId)
              .catch((failure) =>
                setError(
                  String(
                    failure instanceof Error ? failure.message : failure,
                  ).replace(
                    /^Error invoking remote method '[^']*': (Error: )?/,
                    "",
                  ),
                ),
              )
          }
        >
          Land
        </button>
      )}
      {notice.kind === "done" && error && (
        <span className="bubble-error">{error}</span>
      )}
      <button
        className="bubble-open"
        onClick={() => void bridge?.open(notice.taskId, notice.focus)}
      >
        Open
      </button>
    </div>
  );
}

/** The desktop mascot: the sushi with a speech bubble for the newest
 * queued orchd notice, and a +N count with arrows for the rest. */
export function Mascot() {
  const [notices, setNotices] = useState<MascotNotice[]>([]);
  const [index, setIndex] = useState(0);
  const [hops, setHops] = useState(0);
  const [typing, setTyping] = useState<Typing>({ id: "", on: false });
  const rootRef = useRef<HTMLDivElement>(null);
  const lastHeight = useRef(0);

  useEffect(
    () =>
      window.mascot?.onNotices((next) => {
        setNotices((previous) => {
          if (next[0]?.id !== previous[0]?.id) {
            setIndex(0);
            setHops((count) => count + 1);
          }
          return next;
        });
      }),
    [],
  );

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
  const extra = notices.length - 1;
  const step = (delta: number) =>
    setIndex(
      (Math.min(index, notices.length - 1) + delta + notices.length) %
        notices.length,
    );
  const mood = mascotMood(shown, isTyping(typing, shown));
  return (
    <div className="mascot" ref={rootRef}>
      <div className="bubble-wrap">
        <Bubble
          key={`${shown.id}:${shown.kind !== "core-update" && shown.answered}`}
          notice={shown}
          onTyping={setTyping}
        />
        {extra > 0 && (
          <div className="queue">
            <button aria-label="Previous notice" onClick={() => step(-1)}>
              <ChevronLeft size={13} />
            </button>
            <span>+{extra}</span>
            <button aria-label="Next notice" onClick={() => step(1)}>
              <ChevronRight size={13} />
            </button>
          </div>
        )}
      </div>
      <div className={`sushi-hop hop-${hops % 2}`}>
        <Character mood={mood} />
      </div>
    </div>
  );
}
