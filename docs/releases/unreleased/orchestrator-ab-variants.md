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
- A/B runs of an earlier per-turn-skills experiment flag showed that
  injecting repo skills into task agents and letting Jev pick a task's MCP
  servers added work instead of saving it, while trimming the fixed prompt
  prefix cut the first turn by about 30% with no downside. The trim is now
  the unconditional default for every Claude implement run (never review or
  plan): its prompt drops the subagent and other delegation tools via
  `--disallowedTools`, which took the fixed prompt prefix from about 29.9k
  to 19.6k tokens in one measurement. Skills are no longer injected into a
  running agent at all, and the flag itself is gone; `mcp.json` again
  carries the same servers it did before the flag existed.
- `orchd ab` has two more columns: the median first-turn prompt tokens of
  fresh Claude implement attempts, and mean cache-read tokens per implement
  attempt.
- `plannerRoute` and `tierRoutes` (e.g. `{"hard": "claude-sonnet"}`) pick
  the planner's route and a tier's implement route for one task instead of
  the settings' `planner` and `tiers`, so model choice can be A/B'd; an
  unknown route id fails `task.create`, and the task's decisions note the
  override.
- `leanOutput: true` gives a Claude implement run a PreToolUse hook
  (`orchd hook rtk`) that offers `rtk rewrite`'s shorter form of a `Bash`
  command before it runs (only for build, test and lint commands, whose
  output rtk condenses without changing the exit code: never a file read,
  a search, a listing or a command that redirects output), and caps a
  command's inline output at 10,000 characters (`bashOutputMaxChars`; the
  rest is saved to a file the agent can read). The hook needs no socket or
  token -- it never contacts the daemon -- and never rewrites a command
  containing `git commit` or `git push`, so a rewrite can never launder past
  those same deny rules. Codex runs and review/plan sessions are unaffected
  either way.
- `reviewBlind: true` leaves the implementer's summary and decisions out of
  the review brief; the reviewer sees the diff, criteria, verify results and
  screenshots only.
- `advisor: true` makes one read-only call on the planner's route after an
  implement attempt fails and before the retry (skipped when the retry runs on
  that same route): a diagnosis and next step of at most 1500 characters,
  shown to the next attempt under `## Advisor` and stored as the failed
  attempt's `advice`. Its cost counts toward the task, and a failed advisor
  run never blocks the retry.
- `orchd eval run --data <dir> --socket <sock> --set orchd/evals/set-1.json --variant '<json>'`
  (also `--only a,b`, `--repeat N`, `--arms '[<json>, <json>]'`) creates one
  task per set task and arm, each starting at the set's resolved `base` and
  tagged `evalSet`/`evalName`; `--request '<text>' --arms '[...]' [--base <ref>]`
  creates an ad-hoc A/B pair on one real request. It only creates tasks; the
  daemon runs them. `orchd ab --eval <set>` limits the report to that set and
  adds a per-task, per-variant table of cost and attempts. `orchd ab` also
  gets columns for how an implement attempt's tool calls split into process,
  evidence, verify, task and explore work (a keyword heuristic over
  `events.jsonl`), and the tool calls per attempt. Common build/test tools and
  the task's own verify commands count as verify; repository-specific process
  and evidence paths come from `settings.workBuckets` (empty by default).
- `task.create` with a title and goal now starts the task by default, like
  the request form; pass `start: false` to leave it stopped. Before, such a
  task sat `queued` until something called `task.start` or the daemon
  restarted.
- A Claude run that ends before the CLI reports its cost (stopped, stalled,
  or cut off by a daemon or app restart) no longer loses its spend: the
  attempt's cost is priced from the per-message token usage already in its
  `events.jsonl` (each message counted once) and marked `costEstimated`.
  The same holds for a review run (`reviewCostUsd`) and an advisor run
  (new `advisorCostUsd` on the failed attempt; a failure whose advisor run
  was already paid for is not advised again). A plan stage's first run no
  longer drops out of the cost when the planner is asked a second time.
  `settings.prices` now has list prices for `claude-opus-5-5`,
  `claude-sonnet-5-5`, `claude-sonnet-5` and `claude-haiku-4-5` (a dated
  model id matches its family), and a price can carry a `cacheWrite` rate.
  Codex pricing is unchanged.
- `maxCostUsd` (0 = off, the default) is a dollar budget per task. Before
  each plan, advisor, implement or review run, a task that has already spent
  its budget waits with the question "raise or stop" instead of running;
  `raise` adds the same amount again and continues, and the stop is
  recorded in the task's decisions. A run already going is never cut short.
- Every orchd run (plan, implement, review, advisor, audit) records what it
  really used: the model ids the harness reported (Claude's init model and
  `modelUsage` keys, Codex's model), the harness version and a short hash of
  the inputs orchd controls, as `fingerprint` on the attempt. `orchd ab` and
  `orchd eval` split a variant's rows by real model and harness version and
  say so when one variant mixes them; the panel shows `model (version)` on
  the attempt line. Older attempts show `-`.
- `batchQuestions: true` lets the planner mark each question with a
  `recommended` option, its `evidence` and whether it is `blocking` (only
  for irreversible or consequential choices: data loss, a public API or
  contract, money, security). A non-blocking question is not asked: its
  recommendation is recorded as an assumption on the task and planning goes
  on. Blocking questions still go to triage, but reach the owner as one
  question listing each with its recommended option and one free-text
  answer. `task.overturn {id, index, answer}` (the task detail's Overturn
  action) records the owner's answer against an assumption and delivers it
  to the task's next attempt as a message; on a finished task it only
  records. `orchd ab` gets assumptions per task and overturn rate columns.
