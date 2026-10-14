## orchd ends a failing task with a report instead of a question

- A timeout, a flaky check, an install error or a failing test outside the task's diff no longer counts as "the same failure again".
- "Attempts keep failing" is never an open question now. After the policy's continue and the orchestrator's continues, orchd changes the approach once (a higher tier when the settings define one, and the planner re-plans the rest of the work), then marks the task failed. The owner gets one notice with a report, and the overseer is woken when auto-answer is on.
- A review that gives no verdict is tried on another route; if none reads the attempt, the attempt is committed and marked unreviewed. Nobody is asked to approve it.
- The judge for an impossible criterion runs even after the policy answered three other questions. The policy budget is per kind of question and cause. A dropped criterion names who dropped it, and the report lists it as not done.
- A planner that times out or errors is resumed once with twice the time before the task fails. A worktree or setup step that fails is tried again twice with a growing pause.
- A dependency that failed on a timeout or the environment is restarted once more by itself after a pause. Dropping a dependency stays the owner's choice.
