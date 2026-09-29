import {
  formatCost,
  formatDuration,
  implementAttemptCount,
  latestAttempt,
  latestImplementAttempt,
  reviewOf,
  stageTrack,
  totalDurationMs,
} from "./helpers.ts";
import { elapsedLabel } from "./ownerAttention.ts";
import type { Attempt, Task, TimelineSegment, TimelineStage } from "./types";

/** The shape of one line of a segment's narration: what an icon and a tone
 * say before the words do. */
export type NarrationKind = "run" | "ok" | "fail" | "next" | "note";
export type NarrationStep = { kind: NarrationKind; text: string };

/** Pipeline order of the per-stage rows. */
const ROW_ORDER: TimelineStage[] = [
  "plan",
  "implement",
  "wait",
  "verify",
  "review",
  "advisor",
  "final",
];

export type StageRow = {
  stage: TimelineStage;
  label: string;
  detail: string;
  ms: number;
  costUsd: number;
};

function segmentMs(segment: TimelineSegment): number {
  return Math.max(0, segment.endedAt - segment.startedAt);
}

function plural(count: number, word: string): string {
  return `${count} ${word}${count === 1 ? "" : "s"}`;
}

function clip(text: string, max: number): string {
  const line = text.trim().split("\n")[0] ?? "";
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

function attemptOfSegment(
  task: Task,
  segment: TimelineSegment,
): Attempt | undefined {
  return task.attempts.find((attempt) => attempt.n === segment.attempt);
}

export function stageWord(stage: TimelineStage): string {
  return stage === "wait" ? "waiting" : stage;
}

/** `verify · attempt 1`, or `waiting` for the time spent on the owner. */
export function segmentLabel(segment: TimelineSegment): string {
  if (segment.stage === "wait") return "waiting";
  return segment.attempt > 0
    ? `${segment.stage} · attempt ${segment.attempt}`
    : segment.stage;
}

function clock(ms: number): string {
  const date = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** `3m 12s · $0.00 · 20:14–20:17`. */
export function segmentFacts(segment: TimelineSegment): string {
  return [
    formatDuration(segmentMs(segment)),
    formatCost(segment.costUsd),
    `${clock(segment.startedAt)}–${clock(segment.endedAt)}`,
  ].join(" · ");
}

/** The text inside a bar segment (`implement 9m`), only when the segment is
 * wide enough to hold it. */
export function barLabel(
  segment: TimelineSegment,
  totalMs: number,
): string | null {
  if (segment.stage === "wait" || totalMs <= 0) return null;
  if (segmentMs(segment) / totalMs < 0.12) return null;
  return `${segment.stage} ${formatDuration(segmentMs(segment)).split(" ")[0]}`;
}

/** The segment the card opens on: the failed one, else nothing. */
export function defaultSegment(segments: TimelineSegment[]): number | null {
  const failed = segments.findIndex((segment) => segment.failureKind);
  return failed < 0 ? null : failed;
}

/** What the agent did in one segment, composed from the attempt's verify
 * results, failure, review and advice - the daemon keeps no log to replay. */
export function segmentNarration(
  task: Task,
  segment: TimelineSegment,
): NarrationStep[] {
  const attempt = attemptOfSegment(task, segment);
  const steps: NarrationStep[] = [];
  const add = (kind: NarrationKind, text: string) => {
    if (text) steps.push({ kind, text });
  };
  switch (segment.stage) {
    case "plan":
      add("note", attempt?.summary ? clip(attempt.summary, 160) : "");
      break;
    case "implement": {
      const buckets = segment.buckets;
      if (buckets && buckets.explore > 0)
        add("run", `read the code, ${plural(buckets.explore, "call")}`);
      if (buckets && buckets.process > 0)
        add("run", `ran ${plural(buckets.process, "command")}`);
      const files = attempt?.changedFiles ?? [];
      if (files.length > 0)
        add(
          "note",
          `edited ${plural(files.length, "file")} · ${files.slice(0, 2).join(", ")}${files.length > 2 ? "…" : ""}`,
        );
      add("note", attempt?.summary ? clip(attempt.summary, 160) : "");
      break;
    }
    case "verify": {
      const results = attempt?.verify ?? [];
      const failed = results.filter((result) => result.code !== 0);
      if (results.length > 0)
        add(
          "run",
          `ran ${results
            .slice(0, 2)
            .map((result) => result.command)
            .join(
              ", ",
            )}${results.length > 2 ? ` and ${results.length - 2} more` : ""}`,
        );
      for (const result of failed.slice(0, 2)) {
        const last = result.tail
          .split("\n")
          .map((line) => line.trim())
          .filter(Boolean)
          .pop();
        add(
          "fail",
          `${result.command} exited ${result.code ?? "?"}${last ? ` · ${clip(last, 100)}` : ""}`,
        );
      }
      const failure = attempt?.failure;
      if (
        failure &&
        ["verify", "heldout", "evidence"].includes(failure.kind) &&
        failed.length === 0
      )
        add("fail", clip(failure.detail, 160));
      if (segment.failureKind) {
        const retried = task.attempts.some(
          (other) => other.stage === "implement" && other.n > segment.attempt,
        );
        if (retried)
          add(
            "next",
            attempt?.advice
              ? "sent back to implement with the advisor's note"
              : "sent back to implement with the failing output",
          );
      } else if (results.length > 0) add("ok", "every check passed");
      break;
    }
    case "review": {
      const review = attempt?.review ?? reviewOf(task);
      if (review) {
        add(
          review.verdict === "PASS" ? "ok" : "fail",
          `reviewer said ${review.verdict}`,
        );
        for (const finding of review.findings.slice(0, 3))
          add("fail", clip(finding, 160));
        if (review.repeated && review.repeated.length > 0)
          add("note", `${plural(review.repeated.length, "finding")} repeated`);
      }
      break;
    }
    case "advisor":
      add("note", attempt?.advice ? clip(attempt.advice, 200) : "");
      break;
    case "final":
      add(
        "ok",
        task.landedSha
          ? `landed as ${task.landedSha.slice(0, 8)}`
          : "committed the work",
      );
      break;
    case "wait":
      add("note", task.question ? clip(task.question.text, 160) : "");
      break;
  }
  if (steps.length === 0) {
    add(
      segment.failureKind ? "fail" : "note",
      [segment.outcome, segment.failureKind ?? ""].filter(Boolean).join(" · "),
    );
  }
  return steps;
}

function rowDetail(
  task: Task,
  stage: TimelineStage,
  own: TimelineSegment[],
): string {
  const runs = plural(own.length, "run");
  switch (stage) {
    case "plan":
      return runs;
    case "implement": {
      const explored = own.reduce(
        (sum, segment) => sum + (segment.buckets?.explore ?? 0),
        0,
      );
      const edited = latestImplementAttempt(task)?.changedFiles.length ?? 0;
      return [
        runs,
        explored > 0 ? `${explored} files read` : "",
        edited > 0 ? `${edited} edited` : "",
      ]
        .filter(Boolean)
        .join(" · ");
    }
    case "wait":
      return task.question
        ? "waiting for your answer"
        : task.queueReason || "waiting for you";
    case "verify": {
      const failed = own.filter((segment) => segment.failureKind).length;
      return failed > 0 ? `${runs} · ${failed} failed` : runs;
    }
    case "review": {
      const verdict = reviewOf(task)?.verdict;
      const reviewer = task.attempts.find((a) => a.stage === "review");
      const implement = latestImplementAttempt(task);
      const vendor =
        reviewer && implement
          ? reviewer.harness !== implement.harness
            ? "other vendor"
            : "same vendor"
          : "";
      return [vendor, verdict].filter(Boolean).join(" · ") || runs;
    }
    case "advisor": {
      const notes = task.attempts.filter((a) => a.advice).length;
      return notes > 0 ? `${plural(notes, "note")} taken` : runs;
    }
    case "final":
      return task.landedSha ? "merged, worktree removed" : "committed";
  }
}

/** One row per stage that ran, in pipeline order: what it did in a phrase,
 * total time and cost. */
export function stageRows(task: Task, segments: TimelineSegment[]): StageRow[] {
  return ROW_ORDER.flatMap((stage) => {
    const own = segments.filter((segment) => segment.stage === stage);
    if (own.length === 0) return [];
    return [
      {
        stage,
        label: stageWord(stage),
        detail: rowDetail(task, stage, own),
        ms: own.reduce((sum, segment) => sum + segmentMs(segment), 0),
        costUsd: own.reduce((sum, segment) => sum + segment.costUsd, 0),
      },
    ];
  });
}

/** Files the work touched: the latest implement attempt's list. */
export function changedFileCount(task: Task): number {
  return latestImplementAttempt(task)?.changedFiles.length ?? 0;
}

/** `3 files · review PASS`: each part only when the daemon knows it (no diff
 * line counts yet). */
export function reportLine(task: Task): string {
  const files = changedFileCount(task);
  const review = reviewOf(task);
  return [
    files > 0 ? plural(files, "file") : "",
    review ? `review ${review.verdict}` : "",
  ]
    .filter(Boolean)
    .join(" · ");
}

/** The owner's ask: which stage raised it and how long ago. `askedBy` and
 * `askedAt` are read when the daemon sends them; until then the stage comes
 * from the track and the time from the last update. */
export function questionSource(task: Task, now = Date.now()): string {
  const extra = (task.question ?? {}) as { askedBy?: string; askedAt?: number };
  const by =
    extra.askedBy ??
    stageTrack(task).find((step) => step.state === "blocked")?.stage ??
    "implement";
  const minutes = Math.floor(
    (now - (extra.askedAt ?? task.updatedAt)) / 60_000,
  );
  const ago =
    minutes < 1
      ? "just now"
      : `${elapsedLabel(now - (extra.askedAt ?? task.updatedAt))} ago`;
  return `asked by ${by} · ${ago}`;
}

/** The header's facts line after the branch. */
export function headerFacts(
  task: Task,
  maxAttempts: number | undefined,
  now = Date.now(),
): string {
  const count = implementAttemptCount(task);
  const latest = latestAttempt(task);
  const finished = task.status === "done";
  return [
    finished
      ? plural(count, "attempt")
      : `attempt ${count}/${maxAttempts ?? count}`,
    formatDuration(totalDurationMs(task, now)),
    formatCost(task.costUsd),
    !finished && latest ? `${latest.harness} · ${latest.model}` : "",
  ]
    .filter(Boolean)
    .map((part) => `· ${part}`)
    .join(" ");
}

/** `ACCEPTANCE` with the met count only when every criterion has one
 * answer (all met, or the whole task not there yet shows no count). */
export function acceptanceHeading(task: Task, met: boolean): string {
  return met && task.criteria.length > 0
    ? `ACCEPTANCE · ${task.criteria.length}/${task.criteria.length}`
    : "ACCEPTANCE";
}
