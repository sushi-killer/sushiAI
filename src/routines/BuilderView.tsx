import { useEffect, useMemo, useRef, useState } from "react";
import { usePageState } from "./pageState.ts";
import { routinesClient, type BuilderTurn } from "./client.ts";
import { MarkdownDoc } from "../extensions/preview/MarkdownDoc.tsx";
import { Breadcrumb, Card, Composer, MetaLine } from "../ui/index.ts";
import { AskCard } from "./AskCard.tsx";
import {
  actionCounts,
  askReplyText,
  parseAsk,
  readableTurn,
  type AskAction,
  type AskAnswers,
  describeWhen,
  durationText,
  entryKey,
  savePayload,
} from "./model.ts";
import { RunView } from "./RunView.tsx";
import { Scheme } from "./Scheme.tsx";
import { useAccessGrant } from "./useAccessGrant.tsx";
import { refreshRoutines } from "./store.ts";
import type {
  Routine,
  RoutineEntry,
  RoutinesCatalog,
  RunResult,
} from "./types.ts";

/** One message of the builder conversation. */
export type BuilderMessage = {
  role: "you" | "agent";
  text: string;
  /** The answers of an owner card, when this reply came from one. */
  answers?: AskAnswers;
  /** The turn failed: shown as an error, not as a reply. */
  failed?: boolean;
};

export type Build = {
  /** "local" or "ssh:<profile id>": where the builder runs. */
  host: string;
  sessionHost: string;
  /** The first turn's session: the key of the whole conversation. */
  sessionId: string;
  /** The builder's harness; every turn runs headless. */
  harness: string;
  text: string;
  /** The routines that existed before this build; a new one is its product. */
  known: string[];
  /** When the build edits a routine, its key. */
  target?: string;
};

/** R3 and R4: the builder agent's session on the left (read it, answer it),
 * the routine it made on the right with Where and Agent, a dry run and the
 * switch that turns it on. */
export function BuilderView({
  build,
  entries,
  catalog,
  onDone,
}: {
  build: Build;
  entries: RoutineEntry[];
  catalog: RoutinesCatalog | null;
  onDone(key: string | null): void;
}) {
  const key = `builder:${build.sessionId}`;
  // Kept across tabs: the messages, the session of the newest turn and
  // whether that turn still runs.
  const [log, setLog] = usePageState<BuilderMessage[]>(`${key}:log`, [
    { role: "you", text: build.text },
  ]);
  const [last, setLast] = usePageState(`${key}:last`, build.sessionId);
  const [working, setWorking] = usePageState(`${key}:working`, true);
  const [reply, setReply] = usePageState(`${key}:reply`, "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [dry, setDry] = usePageState<number | null>(
    `builder:${build.sessionId}:dry`,
    null,
  );
  const [dryResult, setDryResult] = usePageState<RunResult | null>(
    `builder:${build.sessionId}:dryResult`,
    null,
  );
  const [brief, setBrief] = useState("");
  const transcript = useRef<HTMLDivElement>(null);
  const agentLast = log[log.length - 1]?.role === "agent";

  // A new message comes into view: the start of the agent's reply (a long one
  // is read from the top), the end while the next turn works.
  useEffect(() => {
    const node = transcript.current;
    if (!node) return;
    const replies = node.querySelectorAll(".rt-reply");
    const reply = replies[replies.length - 1];
    if (agentLast && !working && reply)
      node.scrollTop +=
        reply.getBoundingClientRect().top - node.getBoundingClientRect().top;
    else node.scrollTop = node.scrollHeight;
  }, [log.length, agentLast, working]);

  const built = useMemo(
    () =>
      entries.find((entry) =>
        build.target
          ? entryKey(entry) === build.target
          : !build.known.includes(entryKey(entry)),
      ) ?? null,
    [entries, build],
  );
  const builtKey = built ? entryKey(built) : "";
  const builtHost = built?.host;
  const builtId = built?.routine.id;

  useEffect(() => {
    if (!working) return;
    let live = true;
    const read = async () => {
      let turn: BuilderTurn;
      try {
        turn = await routinesClient.buildTurn(last);
      } catch (reason) {
        if (!live) return;
        setError(reason instanceof Error ? reason.message : String(reason));
        setWorking(false);
        return;
      }
      if (!live || turn.state === "running") return;
      setLog((items) => [
        ...items,
        { role: "agent", text: turn.text, failed: turn.state === "failed" },
      ]);
      setWorking(false);
    };
    void read();
    const timer = setInterval(() => void read(), 700);
    return () => {
      live = false;
      clearInterval(timer);
    };
    // The kept-state setters change every render; the effect follows the turn.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [working, last]);

  useEffect(() => {
    if (!builtHost || !builtId) return;
    let live = true;
    routinesClient
      .show(builtHost, builtId)
      .then((detail) => live && setBrief(detail.brief))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [builtHost, builtId, built?.routine.name]);

  async function act(work: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await work();
      await refreshRoutines();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function send(value: string, answers?: AskAnswers) {
    const text = value.trim();
    if (!text || working) return;
    setReply("");
    setError("");
    try {
      const next = await routinesClient.buildReply(last, text);
      setLog((items) => [...items, { role: "you", text, answers }]);
      setLast(next.sessionId);
      setWorking(true);
    } catch (reason) {
      setReply(text);
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  }

  const routine: Routine | null = built?.routine ?? null;
  const access = useAccessGrant({
    host: built?.host ?? "",
    id: builtId ?? "",
    routine,
    fallback: built?.access,
    busy,
    act,
  });
  const blocked = access.waiting.length > 0;
  const dryRun =
    dry !== null && built?.lastRun?.n === dry ? built.lastRun : null;
  const startDry = () =>
    void act(async () => {
      if (!built || !routine) return;
      setDryResult(null);
      const started = await routinesClient.dryRun(built.host, routine.id);
      setDry(started.run);
    });
  // The card's buttons map to this fixed list and nothing else.
  const runAction = (action: AskAction) => {
    if (action.kind === "dry-run") startDry();
    else if (action.kind === "open-routine") {
      if (builtKey) onDone(builtKey);
    } else void send(action.reply);
  };
  const heading = routine ? (dry !== null ? "Dry run" : "Draft") : "Draft";
  return (
    <div className="rt-builder">
      <Breadcrumb
        trail={[{ label: "Routines", onClick: () => onDone(null) }]}
        current={build.target ? "Edit routine" : "New routine"}
      />
      <div className="rt-build">
        <Card as="section" className="rt-chat" label="Builder">
          <div
            ref={transcript}
            className="rt-transcript"
            aria-label="Builder session"
          >
            {log.map((item, index) => {
              if (item.role === "you")
                return (
                  <p key={index} className="rt-you">
                    {item.text}
                  </p>
                );
              if (item.failed)
                return (
                  <p key={index} role="alert" className="rt-failed">
                    {item.text}
                  </p>
                );
              const { body, ask } = parseAsk(item.text);
              return (
                <div key={index} className="rt-reply">
                  {body && <MarkdownDoc body={readableTurn(body)} />}
                  {ask && (
                    <AskCard
                      ask={ask}
                      answered={log[index + 1]?.answers}
                      active={index === log.length - 1 && !working}
                      disabledKinds={
                        !built
                          ? ["dry-run", "open-routine"]
                          : blocked
                            ? ["dry-run"]
                            : []
                      }
                      onAction={runAction}
                      onSend={(answers) =>
                        void send(askReplyText(ask, answers), answers)
                      }
                    />
                  )}
                </div>
              );
            })}
            {working && <p className="rt-status">Working…</p>}
          </div>
          <Composer
            value={reply}
            label="Message to the builder"
            placeholder="Change anything: “only tasks tagged bot”"
            submitLabel="Send"
            tone="secondary"
            compact
            busy={working}
            busyLabel="Working…"
            onChange={setReply}
            onSubmit={(text) => void send(text)}
          />
        </Card>
        <Card
          as="section"
          raised
          className="rt-built rt-detail"
          label="Routine"
        >
          {!built || !routine ? (
            <MetaLine>
              The routine shows here as soon as the builder saves it.
            </MetaLine>
          ) : (
            <>
              <header className="rt-detail-head">
                <div className="rt-detail-title">
                  <h2>
                    {dry !== null ? `Dry run · ${routine.name}` : routine.name}
                  </h2>
                  <MetaLine single>
                    {dry !== null
                      ? dryStats(dryResult, dryRun)
                      : `${heading} · ${describeWhen(routine.when.cron, routine.when.tz)}`}
                  </MetaLine>
                </div>
              </header>
              {access.notice}
              {dry !== null ? (
                <RunView
                  host={built.host}
                  routineId={routine.id}
                  run={
                    dryRun ?? {
                      n: dry,
                      kind: "dry",
                      status: "running",
                    }
                  }
                  bare
                  busy={busy}
                  onRunAgain={startDry}
                  onResult={setDryResult}
                />
              ) : (
                <Scheme
                  routine={routine}
                  host={built.host}
                  brief={brief}
                  catalog={catalog}
                  busy={busy}
                  onChange={(next) =>
                    void act(async () => {
                      await routinesClient.save(
                        built.host,
                        savePayload(next, brief),
                      );
                    })
                  }
                  onMove={(target) =>
                    target !== built.host &&
                    void act(async () => {
                      await routinesClient.move(built.host, target, routine.id);
                      onDone(null);
                    })
                  }
                />
              )}
              <div className="rt-detail-actions">
                {dry === null && (
                  <button
                    type="button"
                    className="ui-button primary"
                    disabled={busy || blocked}
                    title={blocked ? "Grant the access first" : undefined}
                    onClick={startDry}
                  >
                    Dry run
                  </button>
                )}
                <button
                  type="button"
                  className={`ui-button ${dry === null ? "secondary" : "primary"}`}
                  disabled={busy}
                  onClick={() =>
                    void act(async () => {
                      await routinesClient.setEnabled(
                        built.host,
                        routine.id,
                        true,
                      );
                      onDone(builtKey);
                    })
                  }
                >
                  {dry === null ? "Save" : "Save routine"}
                </button>
                {dry !== null && (
                  <button
                    type="button"
                    className="ui-button ghost"
                    disabled={busy}
                    onClick={startDry}
                  >
                    Run again
                  </button>
                )}
              </div>
            </>
          )}
          {access.sheet}
          {error && (
            <p role="alert" className="rt-error">
              {error}
            </p>
          )}
        </Card>
      </div>
    </div>
  );
}

/** "Read 24 tasks · 38 s · nothing was written": the line under a dry run. */
function dryStats(
  result: RunResult | null,
  run: { startedAt?: number; endedAt?: number } | null,
): string {
  if (!result) return "Running…";
  const { applied, wouldDo } = actionCounts(result);
  const took = run ? durationText(run) : "";
  return [
    `${applied} applied`,
    `${wouldDo} would do`,
    took,
    "nothing was written",
  ]
    .filter(Boolean)
    .join(" · ");
}
