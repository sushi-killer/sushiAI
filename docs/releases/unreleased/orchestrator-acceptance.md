## Orchestrator: stricter, less brittle task acceptance

- A review that returns no verdict no longer passes the attempt: the task
  waits for the owner to approve the commit or ask for another attempt.
- With review `auto`, the hard tier's route reviews every other route's
  work; hard-tier work is reviewed on a different harness.
- Plan and review replies are read in either the fenced or the
  `<sushi-review>`-tag shape, and a ```` ``` ```` quoted inside a JSON string no
  longer cuts a plan short.
- A drafted verify entry that isn't a runnable shell command (a screenshot
  instruction, "only if ..." notes) becomes a criterion for the reviewer
  instead of failing every attempt.
- A failed verify keeps the end of both stdout and stderr, so a retry sees
  which test failed instead of the compile log.
- `task.create` takes `base` to branch a task from another branch; the
  orchestrator can split one feature into parallel tasks on the same base.
- Done tasks no longer show a "Run again" button that did nothing.
