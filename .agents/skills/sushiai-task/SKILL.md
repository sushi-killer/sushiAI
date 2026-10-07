---
name: sushiai-task
description: "Carry an owner's sushiAI task to a verified local commit without supervision: scope it, delegate implementer, functional-qa, reviewer and design-critic lanes, loop on findings, and report back with only the questions the owner must answer. Use for any change bigger than one obvious edit; not for read-only questions or a one-line fix."
---

# sushiAI task loop

The owner hands over a task and leaves. Carry it to a verified local commit.
The owner hears back once: the result, the evidence, and only the questions
nobody else can answer. A plan or a progress note is a checkpoint, not done.

## Roles and models

| Lane                  | Who                | Model   | Owns                                                     |
| --------------------- | ------------------ | ------- | -------------------------------------------------------- |
| lead                  | the main session   | session | scope, briefs, integration, commit, report               |
| `implementer`         | subagent           | sonnet  | one bounded code lane and its tests                      |
| `functional-qa`       | subagent           | sonnet  | proof against the acceptance criteria                    |
| `reviewer`            | subagent           | opus    | fresh correctness and root-cause review of the diff      |
| `design-critic`       | subagent           | opus    | judgement of the screenshots, only when a screen changes |
| wide read-only search | built-in `Explore` | default | locating code across many files                          |

Sonnet does work that something else checks afterwards (tests, review). Opus
does judgement nothing checks after it. The lead stays hands-on: it verifies
every consequential conclusion a lane reports instead of forwarding it.

## Loop

1. **Scope.** Read `AGENTS.md`, `git status -sb`, the files the task touches
   with their callers and tests, and `docs/LESSONS.md`. Reproduce a defect
   through the real entry point before editing. Check why a path is missing
   before restoring it (`git log -p -S <symbol>`). Write acceptance criteria a
   person could check from outside the app.
   **Done when:** every criterion is observable and every touched file is named.
2. **Separate what needs the owner.** Ask only about: the extension manifest
   contract or other public shapes, the security model, `src/app`/`electron/main.cjs`
   structural changes (`AGENTS.md` "Vision & boundaries"), destructive or
   irreversible actions, paid services, pushing, merging, packaging or
   releasing. Resolve everything else with a reasonable assumption and record
   it. When the owner is away, park the question in the report and keep every
   lane that does not depend on it moving.
   **Done when:** each open decision is either assumed and recorded or parked.
3. **Build.** Small or tightly coupled work: the lead, or one `implementer`
   in the main checkout. Independent lanes: one `implementer` each, run with
   worktree isolation. A worktree branches from `main`, not from the lead's
   branch - give the lane the lead's commit SHA and have it start with
   `git checkout -B <lane> <sha>` and link `node_modules` from the main
   checkout. Brief every lane with: the criteria it serves, the files it owns,
   the tests to add, and what it must not touch. Two lanes never own one file.
   **Done when:** `npm run build` exits 0 and each lane's tests pass.
4. **Clean.** Run `$deslop` over the whole diff.
   **Done when:** the diff carries no narration comments, imagined-state guards
   or type laundering.
5. **Prove.** Brief `functional-qa` with the criteria. It follows
   `$sushiai-testing`, and `$ui-evidence` when `src/app`, `src/extensions`,
   `electron/` or any screen changed.
   **Done when:** `npm run ci` exits 0, every criterion maps to a test or a
   reproducible step, and gaps are listed.
6. **Review.** Run `$autoreview` on the final diff.
   **Done when:** every P0/P1 finding is verified against the code and either
   fixed or rejected with a reason.
7. **Judge the look.** Only when a screen changed: brief `design-critic` with
   the screenshot paths and the owner's request, nothing else.
   **Done when:** the verdict is PASS or PASS_WITH_NOTES, or its FAIL is fixed.
8. **Loop.** Each blocking finding goes back to step 3 as a brief carrying its
   evidence; rerun only the proof the fix affects. The same finding failing a
   third time means the approach is wrong: stop patching, write the root cause
   and a different approach into the report.
   **Done when:** no blocking finding is open.
9. **Land locally.** Stage only intended files. One Conventional Commit in
   English with no attribution trailers. A `docs/releases/unreleased/<slug>.md`
   fragment for anything user-visible. A `docs/LESSONS.md` entry, or the
   report says there was none. Never push, merge, package or run `ship-pr` /
   `cut-release` unless the owner asked in this task.
   **Done when:** `git status -sb` shows only unrelated work left.
10. **Report** in the shape of [references/report.md](references/report.md).

## Pace

These rules keep a task from growing into days. Source: the project-environments
retro (`artifacts/RETRO-project-environments.md`).

- **Cutline.** After step 1 write the done list (about eight items) and a time
  budget, and tell the owner the ETA. A new ask goes to a `Next` list unless the
  owner marks it blocking. Update the ETA after every pass; never answer "how is
  it going" with only "still running".
- **Ask consent and security UX before building.** Default to the lowest-friction
  choice for owner-owned resources (a host the owner added is trusted). Never
  build a second mechanism for a decision the owner already made.
- **Two gate tiers.** Inner loop: targeted tests and only the affected screens.
  Final gate once: `prettier`, build, `npm run ci`, `npm run test:desktop`, the
  full screen pass. Revert-red checks run once at the end, only for tests that
  guard security or data.
- **Review budget.** One full `reviewer` pass and one delta pass. The second
  reports P0/P1 only; P2 goes to the report's "Left open" unless it is a
  security issue. `design-critic`: one full pass, one delta pass.
- **Fixed fidelity tolerances**, set by the lead before the run (text width 3%,
  shifts 2 px, no data-list excuses for layout). The critic, not a diff tool the
  implementer wrote, decides.
- **Parallel lanes** for independent surfaces (electron/orchestrator module, renderer, docs and
  Figma), each in its own worktree.
- **Preflight (two minutes):** Figma bridge connected, ssh alias answers, the
  build runs, the ports you need are free. Fix this before briefing a lane.

## Boundaries that hold in every lane

- Never stop, restart or reconfigure a process, dev server or app you did not
  start; never free a port by force.
- Tests and examples use synthetic data: `192.0.2.0/24`, `user@devbox`,
  invented names. Real hosts, IPs, paths and credentials stay out of files,
  commits and screenshots.
- One owner, complete cutover (`AGENTS.md`): remove the old path in the same
  change.
- The exit code is the verdict, not a summary line. Use `npm run ci` or
  `./node_modules/.bin/<tool>`, never `npx`.
