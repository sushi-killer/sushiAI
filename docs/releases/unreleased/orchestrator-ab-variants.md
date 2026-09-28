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
- Task costs now include the plan run and the review run, which were
  reported by the CLI but never added. Codex runs, which report tokens but
  no cost, are priced from `settings.prices` (list prices for
  `gpt-5.3-codex` and `gpt-5.6-luna` built in).
- `orchd ab` leaves out tasks created before variants existed, so old runs
  don't skew the default arm.
- `contract: true` has the planner check the request's claims against the
  code and pair every criterion with a check a read-only reviewer can do,
  and has the reviewer rule on each criterion: met, unmet, or not
  checkable. Any review reply that marks a criterion unmet fails the
  attempt, even one that says PASS; a criterion it could not check is only
  noted.
- `reviewOtherFamily: true` (with review `auto`) reviews on the other
  harness when a route there exists: Claude's work by Codex and Codex's by
  Claude. When none exists the task says so in its decisions.
- `reviewEvidence: true` shows the reviewer the screenshots the attempt
  saved under `artifacts/` (attached to a Codex reviewer's prompt, listed
  for a Claude reviewer to open) and longer verify output.
- `deferHeavyChecks: true` has the planner split slow whole-repo checks (the
  full CI script, a desktop smoke) into `finalVerify`. Agents and the Stop
  hook run only the fast `verify`; orchd runs `finalVerify` once, after
  review passes and before the commit, and a failure there is a normal
  retry. `task.create` also takes `finalVerify` directly.
- `leanContext: true` gives implement agents the skills and MCP servers Jev
  picks for the task instead of a fixed set. A Claude agent gets a skill's
  `SKILL.md` body through a hook when its prompt or a tool's output calls
  for it: at most 3 per call, each once per session, and never a skill
  marked `disable-model-invocation`. A fresh Codex attempt gets the picked
  skills in its brief. A Claude run's MCP servers are cut to the ones Jev
  picks (orchd's messaging server always stays; if Jev can't be reached,
  every server stays), and its prompt drops the subagent and other
  delegation tools, which took the fixed prompt prefix from about 29.9k to
  19.6k tokens in one measurement.
- `orchd ab` has two more columns: the median first-turn prompt tokens of
  fresh Claude implement attempts, and mean cache-read tokens per implement
  attempt.
