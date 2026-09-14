---
name: autoreview
description: "Fresh-context independent code review of a sushiAI diff before it is committed: correctness, root cause over symptom, callers and sibling paths, security boundary, test value. Use after implementation and deslop, before landing any nontrivial change, or when the owner asks for a review."
---

# Autoreview

Findings are hypotheses to verify, not instructions to apply.

## Run

1. **Pick the target.** Uncommitted work: `git diff HEAD` plus untracked files.
   A branch: `git diff $(git merge-base main HEAD)...HEAD`. One commit:
   `git show <sha>`.
   **Done when:** the reviewer is given one exact command that reproduces the diff.
2. **Brief the `reviewer` subagent** with the owner's request, the acceptance
   criteria, the diff command and the severity threshold (P0-P1 by default;
   P2-P3 only when asked). Do not tell it what you believe is wrong or why
   the code is right - its value is a fresh read.
   **Done when:** the brief carries no diagnosis.
3. **Second engine for risky diffs.** When the diff touches the renderer
   security settings, IPC handlers, the extension manifest validator,
   persistence or SSH, also run Codex read-only if it is installed
   (`command -v codex`): check the flags with `codex exec --help`, then run
   `codex exec --sandbox read-only` with the same brief. Skip silently when it
   is absent.
4. **Verify every finding** against the actual code before touching anything.
   Fix what is real; reject the rest with a one-line reason in the report.
   **Done when:** each P0/P1 is fixed or rejected with evidence.
5. **Re-review only substantive fixes.** No extra rounds for a nicer verdict.

## Severity

- **P0** - breaks normal operation, loses data, crosses the security boundary,
  or leaks a credential or real private data (report it without repeating the
  value).
- **P1** - wrong behaviour on a realistic path, a missed caller or sibling, a
  test that cannot fail.
- **P2** - maintainability or clarity cost with a concrete consequence.
- **P3** - style.
