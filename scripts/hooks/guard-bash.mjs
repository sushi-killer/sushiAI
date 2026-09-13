#!/usr/bin/env node
// PreToolUse/Bash. Two kinds of rule live here:
// - a reminder for commands whose cost isn't obvious from reading them
//   (ask, then let the normal permission flow decide);
// - a hard deny for merging/pushing to main, because GitHub branch
//   protection can't tell "the owner clicked merge" apart from "the agent
//   ran gh/git using the owner's own credentials" - only this process can
//   refuse to be the one that pressed the button. See AGENTS.md "Release
//   and branching": only the owner merges, no exceptions.
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";

const input = JSON.parse(readFileSync(0, "utf8"));
const rawCommand = input.tool_input?.command ?? "";
// A commit message written via `git commit -m "$(cat <<'EOF' ... EOF)"` can
// legitimately *mention* a trigger phrase (documenting what this hook
// denies, say) without the command actually invoking it. Strip heredoc
// bodies before scanning, so prose inside one can't trip a deny meant for
// commands actually being run.
const command = rawCommand.replace(
  /<<[-~]?\s*(['"]?)(\w+)\1[\s\S]*?\n\2\b/g,
  "",
);

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

const deny = (reason) => {
  console.log(
    JSON.stringify({
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: "deny",
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

// Merging is exclusively the owner's action, PR or release PR alike.
if (/\bgh\s+pr\s+merge\b/.test(command))
  deny(
    "Only the owner merges a PR - never the agent, even with matching credentials. Ask the owner to merge it themselves.",
  );

// Any push that lands on main directly, bypassing the PR pipeline entirely.
if (/\bgit\s+push\b/.test(command)) {
  let targetsMain = /(^|[\s:])main(\s|$)/.test(command);
  if (!targetsMain && !command.includes(":")) {
    // No explicit refspec (a bare "git push", "git push origin", or
    // "git push -u origin <branch>" tracking the branch itself) - the
    // destination is whatever's currently checked out.
    try {
      const branch = execFileSync(
        "git",
        ["rev-parse", "--abbrev-ref", "HEAD"],
        { cwd: input.cwd, encoding: "utf8" },
      ).trim();
      targetsMain = branch === "main";
    } catch {
      // Can't tell - don't block a command this couldn't parse.
    }
  }
  if (targetsMain)
    deny(
      'Direct pushes to main are not allowed. Branch, open a PR (see the "ship-pr" skill), and let the owner merge it.',
    );
}

process.exit(0);
