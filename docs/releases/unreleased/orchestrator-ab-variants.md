## Orchestrator: A/B experiment flags

- Every task records the experiment flags it runs with (`variant`).
  `settings.experiments` sets the defaults, and `task.create` (and the
  orchestrator's `task_create` tool) can override them per task, so the same
  task can run twice side by side under two settings.
- `orchd ab --data <dir>` prints one row per variant: tasks, done, attempts
  per task, mean and median cost, median minutes to done, review FAILs,
  owner answers per task and stalls.
- `retryMode: "fresh"` starts a retry as a new session with the full brief,
  each earlier attempt's handoff note and the whole latest failure, instead
  of resuming the failed session. Agents are now asked to end every report
  with a `handoff` note (what is done, what was tried, what to do next);
  only the fresh arm reads it back.
- `stallTimeoutSecs` stops an implement attempt whose agent prints nothing
  for that long and retries it; the failure shows as `stall`. The clock
  waits while orchd runs the Stop-hook verify, but an agent's own long
  command is silent too, so keep it at 15 minutes or more.
- `plannerTier: true` routes a planned task by the tier the planner chose,
  and uses Jev only when the plan has none. The planner's choice is recorded
  as `plannedTier` either way, for comparison with Jev's.
- All flags are off by default. The only change without a flag: the
  report and plan formats now ask for `handoff` and `tier`.
