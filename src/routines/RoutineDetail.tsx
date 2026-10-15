import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronRight } from "lucide-react";
import type { CoreViewProps } from "../extensions/coreViews.ts";
import { Card, Dot, Eyebrow, MetaLine, Notice, Toggle } from "../ui/index.ts";
import { AccessSheet } from "./AccessSheet.tsx";
import { routinesClient } from "./client.ts";
import {
  describeWhen,
  routineState,
  runDate,
  runRowTitle,
  savePayload,
  STATUS_LABELS,
  statusTone,
  ungrantedAccess,
} from "./model.ts";
import { LoginButton } from "./LoginButton.tsx";
import { RunView } from "./RunView.tsx";
import { Scheme } from "./Scheme.tsx";
import type {
  Routine,
  RoutineDetail as Detail,
  RoutineEntry,
  RoutinesCatalog,
  RunSummary,
} from "./types.ts";
import { refreshRoutines } from "./store.ts";

/** R2's right side: one routine - what it does, how it works, its last runs.
 * A review, a failure and a run open inside it, never on another page. */
export function RoutineDetail({
  entry,
  catalog,
  openSession,
  onEdit,
  onGone,
  onDoc,
}: {
  entry: RoutineEntry;
  catalog: RoutinesCatalog | null;
  openSession?: CoreViewProps["openSession"];
  /** "Edit with builder": a builder session on this routine. */
  onEdit(entry: RoutineEntry): void;
  onGone(): void;
  /** "Open as doc": the result of one run as a document page. */
  onDoc(entry: RoutineEntry, run: RunSummary): void;
}) {
  const { host } = entry;
  const id = entry.routine.id;
  const [detail, setDetail] = useState<Detail | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [openRun, setOpenRun] = useState<number | null>(null);
  const [sheet, setSheet] = useState(false);
  const last = entry.lastRun;
  const revision = `${last?.n ?? 0}:${last?.status ?? ""}:${entry.routine.enabled}`;

  const load = useCallback(async () => {
    try {
      setDetail(await routinesClient.show(host, id));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  }, [host, id]);
  useEffect(() => {
    void load();
  }, [load, revision]);
  useEffect(() => setOpenRun(null), [host, id]);

  async function act(work: () => Promise<unknown>) {
    setBusy(true);
    setError("");
    try {
      await work();
      await Promise.all([load(), refreshRoutines()]);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  const routine: Routine = detail?.routine ?? entry.routine;
  const waiting = useMemo(
    () => ungrantedAccess(routine, detail?.access ?? entry.access),
    [routine, detail?.access, entry.access],
  );
  // The sheet opens by itself once for each list of tools that waits.
  const waitingKey = waiting.map((item) => item.tool).join("\n");
  useEffect(() => setSheet(waitingKey !== ""), [id, waitingKey]);

  const state = routineState({ ...entry, routine });
  const runs: RunSummary[] = detail?.runs ?? [];
  const shown = openRun === null ? null : runs.find((run) => run.n === openRun);
  const runNow = () =>
    void act(async () => {
      const fired = await routinesClient.runNow(host, id);
      if (fired.skipped) throw new Error(`Skipped: ${fired.skipped}`);
      setOpenRun(null);
    });

  const needsNotice =
    state === "needs_you" || state === "failed" || state === "paused";
  const noticeRun = last && last.kind !== "dry" ? last : null;
  const noticeText =
    state === "needs_you"
      ? noticeRun?.title || "Waiting for you"
      : noticeRun?.reason || noticeRun?.title || "The last run failed";

  const head = (
    <header className="rt-detail-head">
      <div className="rt-detail-title">
        <h2>{routine.name}</h2>
        <MetaLine single>
          {describeWhen(routine.when.cron, routine.when.tz)}
        </MetaLine>
      </div>
      <Toggle
        checked={routine.enabled}
        label="On"
        disabled={busy}
        onChange={(value) =>
          void act(() => routinesClient.setEnabled(host, id, value))
        }
      />
    </header>
  );

  return (
    <Card raised className="rt-detail">
      {shown ? (
        <RunView
          host={host}
          routineId={id}
          routineName={routine.name}
          run={shown}
          busy={busy}
          trust={trustLine(routine)}
          openSession={openSession}
          onBack={() => setOpenRun(null)}
          onRunAgain={runNow}
          head={head}
          onAsk={() => onEdit(entry)}
          onDoc={() => onDoc(entry, shown)}
          onChanged={() => void act(async () => {})}
        />
      ) : (
        <>
          {head}
          {error && (
            <p role="alert" className="rt-error">
              {error}
            </p>
          )}
          {waiting.length > 0 && (
            <Notice
              title={`Needs access to ${waiting.map((item) => item.label).join(", ")}`}
              detail="Granted to this routine only."
              action={
                <button
                  type="button"
                  className="ui-button ghost"
                  onClick={() => setSheet(true)}
                >
                  Review access
                </button>
              }
            />
          )}
          {needsNotice && (
            <Notice
              tone={state === "needs_you" ? "warning" : "danger"}
              title={noticeText}
              detail={
                state === "needs_you"
                  ? undefined
                  : "Fix it, then run the routine again."
              }
              action={
                <span className="rt-notice-actions">
                  {openSession &&
                    noticeRun?.loginHost &&
                    noticeRun.loginHarness && (
                      <LoginButton
                        host={host}
                        loginHost={noticeRun.loginHost}
                        harness={noticeRun.loginHarness}
                        openSession={openSession}
                      />
                    )}
                  {noticeRun && (
                    <button
                      type="button"
                      className="ui-button ghost"
                      onClick={() => setOpenRun(noticeRun.n)}
                    >
                      View
                    </button>
                  )}
                </span>
              }
            />
          )}
          <Scheme
            routine={routine}
            host={host}
            brief={detail?.brief ?? ""}
            catalog={catalog}
            busy={busy || !detail}
            hideMode={state === "failed" || state === "paused"}
            onChange={(next) =>
              void act(() =>
                routinesClient.save(
                  host,
                  savePayload(next, detail?.brief ?? ""),
                ),
              )
            }
            onMove={(target) =>
              target !== host &&
              void act(async () => {
                await routinesClient.move(host, target, id);
                onGone();
              })
            }
          />
          <section className="rt-scheme" aria-label="Last runs">
            <Eyebrow>Last runs</Eyebrow>
            {runs.length === 0 ? (
              <MetaLine>No runs yet.</MetaLine>
            ) : (
              <ul className="rt-runs">
                {runs.slice(0, 10).map((run) => (
                  <li key={run.n}>
                    <button type="button" onClick={() => setOpenRun(run.n)}>
                      <Dot
                        tone={statusTone(run.status)}
                        label={STATUS_LABELS[run.status]}
                      />
                      <span className="rt-run-date">
                        {runDate(run.startedAt) || `Run ${run.n}`}
                      </span>
                      <span className="rt-run-title">{runRowTitle(run)}</span>
                      <ChevronRight size={12} aria-hidden />
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </section>
          <div className="rt-detail-actions">
            <button
              type="button"
              className="ui-button secondary"
              disabled={busy || state === "running"}
              onClick={runNow}
            >
              Run now
            </button>
            <button
              type="button"
              className="ui-button ghost"
              disabled={busy}
              onClick={() => onEdit(entry)}
            >
              Edit with builder
            </button>
            <button
              type="button"
              className="ui-button ghost rt-delete"
              disabled={busy}
              onClick={() =>
                void act(async () => {
                  await routinesClient.remove(host, id);
                  onGone();
                })
              }
            >
              Delete
            </button>
          </div>
        </>
      )}
      {sheet && waiting.length > 0 && (
        <AccessSheet
          name={routine.name}
          items={waiting}
          busy={busy}
          onCancel={() => setSheet(false)}
          onAllow={() =>
            void act(async () => {
              await routinesClient.grant(
                host,
                id,
                waiting.map((item) => item.tool),
              );
              setSheet(false);
            })
          }
        />
      )}
    </Card>
  );
}

/** What is left before a routine acts on its own. */
function trustLine(routine: Routine): string {
  if (routine.howItRuns.mode !== "ask-first" || !routine.howItRuns.promoteAfter)
    return "";
  const left = Math.max(
    0,
    routine.howItRuns.promoteAfter - routine.howItRuns.accepted,
  );
  return `${left} more accepted ${left === 1 ? "run" : "runs"} and it acts on its own`;
}
