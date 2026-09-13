#!/usr/bin/env node
// Stop: nudge toward a docs/LESSONS.md entry, but only for actual trouble.
// Two stages: a cheap, turn-scoped pre-filter (this script), then a Haiku
// judgment call on what the pre-filter catches — the pre-filter alone
// cannot tell "planned work touched this file three times" from "we were
// stuck on this file," so it used to fire on ordinary multi-step sessions.
// Stage 2 fires rarely (only when stage 1 does) and is what actually
// decides; stage 1 just keeps the common case free.
import { readFileSync, existsSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";
import os from "node:os";

// The stage-2 call below is itself a `claude -p` run in this project, so it
// would trigger this very hook again when it finishes. This breaks that loop.
if (process.env.SUSHIAI_LESSON_REMINDER_NESTED) process.exit(0);

const input = JSON.parse(readFileSync(0, "utf8"));
if (input.stop_hook_active) process.exit(0);

const transcriptPath = input.transcript_path;
const sessionId = input.session_id;
if (!transcriptPath || !existsSync(transcriptPath) || !sessionId)
  process.exit(0);

// Only the cursor persists across turns - not edit counts. Each invocation
// only sees lines since the last Stop, which is naturally "this turn"
// (Claude Code calls Stop once per turn), so churn is scoped for free.
const statePath = path.join(
  os.tmpdir(),
  `sushiai-lesson-reminder-${sessionId}.json`,
);
let scannedLines = 0;
if (existsSync(statePath)) {
  try {
    scannedLines =
      JSON.parse(readFileSync(statePath, "utf8")).scannedLines ?? 0;
  } catch {
    // Corrupt or half-written state file: start fresh rather than crash.
  }
}

const lines = readFileSync(transcriptPath, "utf8").split("\n").filter(Boolean);
const newLines = lines.slice(scannedLines);
writeFileSync(statePath, JSON.stringify({ scannedLines: lines.length }));

const entries = [];
for (const line of newLines) {
  try {
    entries.push(JSON.parse(line));
  } catch {
    // Not every line is guaranteed to be a complete JSON object; skip it.
  }
}

let sawError = false;
const editCounts = new Map();
function walk(value) {
  if (!value || typeof value !== "object") return;
  if (Array.isArray(value)) return value.forEach(walk);
  if (value.is_error === true) sawError = true;
  if (
    value.type === "tool_use" &&
    (value.name === "Edit" || value.name === "Write") &&
    typeof value.input?.file_path === "string" &&
    !value.input.file_path.endsWith("docs/LESSONS.md")
  ) {
    const key = value.input.file_path;
    editCounts.set(key, (editCounts.get(key) ?? 0) + 1);
  }
  for (const key of Object.keys(value)) walk(value[key]);
}
for (const entry of entries) walk(entry);

const churned = [...editCounts.entries()].find(([, count]) => count >= 3);
if (!sawError && !churned) process.exit(0);

// Stage 2: build a bounded digest of this turn and ask Haiku whether it's
// actually a lesson, rather than blocking on the pre-filter's say-so alone.
function textOf(content) {
  if (typeof content === "string") return content;
  if (Array.isArray(content))
    return content
      .filter((c) => c?.type === "text")
      .map((c) => c.text)
      .join(" ");
  return "";
}

const digestLines = [];
for (const entry of entries) {
  const msg = entry?.message;
  if (!msg) continue;
  if (msg.role === "user" && typeof msg.content !== "object") {
    digestLines.push(`USER: ${String(msg.content).slice(0, 300)}`);
  }
  for (const block of Array.isArray(msg.content) ? msg.content : []) {
    if (block.type === "text" && block.text)
      digestLines.push(`ASSISTANT: ${block.text.slice(0, 400)}`);
    if (block.type === "tool_use")
      digestLines.push(
        `TOOL_USE: ${block.name} ${JSON.stringify(block.input).slice(0, 120)}`,
      );
    if (block.type === "tool_result") {
      const text = textOf(block.content).slice(0, 200);
      digestLines.push(
        `TOOL_RESULT: ${block.is_error ? "ERROR" : "ok"} ${text}`,
      );
    }
  }
}
const digest = digestLines.join("\n").slice(0, 6000);

// Covers both still-open entries ("## 2026-...") and promoted index lines
// ("- 2026-... -> ..."), while skipping the fenced format example and the
// bare "## Open"/"## Promoted" section headers (neither starts with a date).
const existingHeadings = existsSync("docs/LESSONS.md")
  ? readFileSync("docs/LESSONS.md", "utf8")
      .replace(/```[\s\S]*?```/g, "")
      .split("\n")
      .filter((line) => /^(##|-) \d{4}-\d{2}-\d{2}/.test(line))
      .join("\n")
  : "";

const prompt = `You judge whether a coding-session excerpt contains a lesson worth logging. A lesson exists only if ALL hold: (1) the agent acted on a wrong belief about this repo, its tooling, or its environment; (2) correcting it took >=2 attempts or a visible reversal; (3) the corrected belief is generalizable: a future session in this repo would plausibly repeat the mistake; (4) it is not already covered by the existing lesson headings below. NOT a lesson: planned multi-step edits to one file, a typo fixed on the next try, a first-run test failure in a red-green loop, tool errors from permissions or an empty search, requirement changes by the user. Reply ONLY with JSON: {"lesson":true|false,"symptom":"<=12 words","rule":"one sentence or empty"}.

EXISTING LESSON HEADINGS:
${existingHeadings || "(none yet)"}

SESSION EXCERPT:
${digest}`;

const result = spawnSync(
  "claude",
  [
    "-p",
    prompt,
    "--model",
    "claude-haiku-4-5-20251001",
    "--output-format",
    "json",
  ],
  {
    encoding: "utf8",
    timeout: 40000,
    killSignal: "SIGKILL",
    env: { ...process.env, SUSHIAI_LESSON_REMINDER_NESTED: "1" },
  },
);

if (result.status !== 0 || !result.stdout) process.exit(0); // fail open

let verdict;
try {
  const outer = JSON.parse(result.stdout);
  const match = String(outer.result ?? "").match(/\{[\s\S]*\}/);
  verdict = match ? JSON.parse(match[0]) : null;
} catch {
  process.exit(0); // fail open on any parse trouble
}

if (verdict?.lesson === true) {
  console.log(
    JSON.stringify({
      decision: "block",
      reason: `This session hit a lesson worth logging: ${verdict.symptom}. Append a docs/LESSONS.md entry (suggested rule: ${verdict.rule || "(none suggested — write one)"}) — adjust as needed before committing it, or say explicitly why it doesn't apply.`,
    }),
  );
}
process.exit(0);
