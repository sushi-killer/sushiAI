#!/usr/bin/env node
// PreToolUse/Bash. Two kinds of rule live here:
// - a reminder for commands whose cost isn't obvious from reading them
//   (ask, then let the normal permission flow decide);
// - a hard deny for merging/pushing to main. This raises the bar against
//   the realistic case (the agent just running the command directly) but
//   is NOT a real trust boundary - a determined bypass (a hand-obfuscated
//   command, an unreadable/remote script, eval indirection) can still get
//   around plain-text scanning, and this only covers the Bash tool.
// See AGENTS.md "Release and branching": the actual guarantee is that this
// agent doesn't try to route around its own stated policy, not that no
// command string could ever slip past a regex.
import { readFileSync, existsSync } from "node:fs";
import { execFileSync } from "node:child_process";
import path from "node:path";

const input = JSON.parse(readFileSync(0, "utf8"));
const rawCommand = input.tool_input?.command ?? "";
const cwd = input.cwd || process.cwd();

// Split on real command boundaries only (;, &&, ||, |, a command-substitution
// start, or a newline) - never by stripping out regions we've decided look
// "safe" (like a heredoc body), which is exactly the kind of regex-vs-real-
// shell mismatch that hides a genuinely executed command from this scan (a
// heredoc piped into `bash`, say). A trigger phrase only counts if it starts
// one of these segments, which is what actually distinguishes an invoked
// command from the same words appearing as prose inside a quoted argument
// (a commit message documenting this very rule, for instance).
function commandSegments(text) {
  return text
    .split(/\n|;|&&|\|\||\||\$\(/)
    .map((segment) => segment.trim())
    .filter(Boolean);
}

// A script file executed by path never shows the dangerous text in the
// command string at all ("bash /tmp/x.sh"). Read and scan it too, for the
// direct-interpreter-invocation case at least - this doesn't reach eval of
// a piped-in remote script or similar deeper indirection.
function allSegments(text) {
  const segments = commandSegments(text);
  const fromScripts = [];
  for (const segment of segments) {
    const match = segment.match(/^(?:bash|sh|zsh|source|\.)\s+(\S+)/);
    if (!match) continue;
    const file = path.isAbsolute(match[1])
      ? match[1]
      : path.join(cwd, match[1]);
    try {
      if (existsSync(file))
        fromScripts.push(...commandSegments(readFileSync(file, "utf8")));
    } catch {
      // Unreadable - nothing more to check here.
    }
  }
  return [...segments, ...fromScripts];
}

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

if (/\bnpm\s+run\s+package\b|\belectron-builder\b/.test(rawCommand))
  ask(
    "This overwrites release/mac-arm64 with no backup (npm run package). Confirm before running.",
  );

if (
  /\b(kill|pkill)\b/.test(rawCommand) ||
  /\blsof\b.*\bxargs\s+kill\b/.test(rawCommand)
)
  ask(
    "This can kill a process this session didn't start. Confirm whose process it is before running.",
  );

const segments = allSegments(rawCommand);

// Merging is exclusively the owner's action, PR or release PR alike.
if (segments.some((segment) => /^gh\s+pr\s+merge\b/.test(segment)))
  deny(
    "Only the owner merges a PR - never the agent, even with matching credentials. Ask the owner to merge it themselves.",
  );

// Any push that lands on main directly, bypassing the PR pipeline entirely.
const pushSegment = segments.find((segment) => /^git\s+push\b/.test(segment));
if (pushSegment) {
  let targetsMain = /(^|[\s:])main(\s|$)/.test(pushSegment);
  if (!targetsMain && !pushSegment.includes(":")) {
    // No explicit refspec (a bare "git push", "git push origin", or
    // "git push -u origin <branch>" tracking the branch itself) - the
    // destination is whatever's currently checked out.
    try {
      const branch = execFileSync(
        "git",
        ["rev-parse", "--abbrev-ref", "HEAD"],
        { cwd, encoding: "utf8" },
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
