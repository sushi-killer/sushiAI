#!/usr/bin/env node
// PreToolUse/Bash: not a policy gate, a reminder. Two commands in this repo
// have a cost that isn't obvious from reading them - surface it once, then
// let the normal permission flow decide.
import { readFileSync } from "node:fs";

const input = JSON.parse(readFileSync(0, "utf8"));
const command = input.tool_input?.command ?? "";

const ask = (reason) => {
  console.log(
    JSON.stringify({
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: "ask",
        permissionDecisionReason: reason,
      },
    }),
  );
  process.exit(0);
};

if (/\bnpm\s+run\s+package\b|\belectron-builder\b/.test(command))
  ask(
    "This overwrites release/mac-arm64 with no backup (npm run package). Confirm before running.",
  );

if (
  /\b(kill|pkill)\b/.test(command) ||
  /\blsof\b.*\bxargs\s+kill\b/.test(command)
)
  ask(
    "This can kill a process this session didn't start. Confirm whose process it is before running.",
  );

process.exit(0);
