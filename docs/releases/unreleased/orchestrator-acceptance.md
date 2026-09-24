## Orchestrator: stricter, less brittle task acceptance

- A review that returns no verdict no longer passes the attempt: the task
  waits for the owner to approve the commit or ask for another attempt.
- With review `auto`, the hard tier's route reviews every other route's
  work; hard-tier work is reviewed on a different harness.
- Plan and review replies are read in either the fenced or the
  `<sushi-review>`-tag shape, and a ` ``` ` quoted inside a JSON string no
  longer cuts a plan short.
- A drafted verify entry that isn't a runnable shell command (a screenshot
  instruction, "only if ..." notes) becomes a criterion for the reviewer
  instead of failing every attempt.
- A failed verify keeps the end of both stdout and stderr, so a retry sees
  which test failed instead of the compile log.
- `task.create` takes `base` to branch a task from another branch; the
  orchestrator can split one feature into parallel tasks on the same base.
- Done tasks no longer show a "Run again" button that did nothing.
- A resumed attempt's cost is its own share of the session: Claude reports
  the session's running total, which was added again on every resume and
  inflated task costs several times over.
- A review reply that is bare JSON, or lists findings as objects, is read as
  a verdict instead of asking the owner.
- Claude task agents can run commands again: the sandbox's network block
  carried keys that made Claude Code deny every Bash call headlessly, so
  agents could not build, test or screenshot. The sandbox now only keeps
  writes inside the task's worktree, and the network is open by default.
- A verify result is no longer reused after the agent edits a file it
  created: untracked files' contents now count toward the cache key.
- When a task's base branch moves ahead while an agent works, orchd carries
  the work onto it before verify, so review and verify see the tree that
  would land. Conflicts come back to the agent to resolve.
- "Attempts keep failing" is answered by the orchestrator first (at most
  twice per task): it can say what to fix or that a review finding is wrong,
  and the owner hears only what it cannot settle.
