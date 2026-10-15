import { useEffect, useState } from "react";
import { MarkdownDoc } from "../extensions/preview/MarkdownDoc.tsx";
import { Breadcrumb, DocBody, DocColumn } from "../ui/index.ts";
import { routinesClient } from "./client.ts";
import { durationText, HARNESSES, runDateTime } from "./model.ts";
import type { RoutineEntry, RunResult, RunSummary } from "./types.ts";

/** R11: the summary of one run as a document - a reading column, no cards. */
export function DocView({
  entry,
  run,
  onEdit,
  onBack,
}: {
  entry: RoutineEntry;
  run: RunSummary;
  /** "Edit": a builder session on the routine. */
  onEdit(): void;
  onBack(): void;
}) {
  const [result, setResult] = useState<RunResult | null>(null);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    let live = true;
    routinesClient
      .result(entry.host, entry.routine.id, run.n)
      .then((value) => live && setResult(value.result))
      .catch(
        (reason) =>
          live &&
          setError(reason instanceof Error ? reason.message : String(reason)),
      );
    return () => {
      live = false;
    };
  }, [entry.host, entry.routine.id, run.n]);
  const harness =
    HARNESSES.find((item) => item.id === entry.routine.agent.harness)?.label ??
    entry.routine.agent.harness;
  const meta = [
    entry.routine.name,
    `${harness} on ${entry.hostName}`,
    durationText(run),
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <div className="rt-page">
      <Breadcrumb
        trail={[
          { label: "Routines", onClick: onBack },
          { label: entry.routine.name, onClick: onBack },
        ]}
        current={runDateTime(run.startedAt) || `Run ${run.n}`}
      />
      <DocColumn
        kind="report"
        meta={meta}
        tools={
          <>
            <button
              type="button"
              className="ui-button ghost"
              disabled={!result}
              onClick={() => {
                if (!result) return;
                void navigator.clipboard?.writeText(result.summary);
                setCopied(true);
              }}
            >
              {copied ? "Copied" : "Copy"}
            </button>
            <button type="button" className="ui-button ghost" onClick={onEdit}>
              Edit
            </button>
          </>
        }
      >
        {error && (
          <p role="alert" className="rt-error">
            {error}
          </p>
        )}
        {result && (
          <DocBody>
            <MarkdownDoc body={result.summary} />
          </DocBody>
        )}
        {result?.next && (
          <DocBody>
            <section aria-label="Suggested next">
              <h2>Suggested next</h2>
              <p>{result.next}</p>
            </section>
          </DocBody>
        )}
        <div className="rt-detail-actions">
          <button
            type="button"
            className="ui-button secondary"
            onClick={onEdit}
          >
            Ask about it
          </button>
        </div>
      </DocColumn>
    </div>
  );
}
