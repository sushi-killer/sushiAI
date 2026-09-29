# orchd: cost date ranges and a daily spend budget

`costs.summary` takes optional `from` and `to` params, UTC dates
`YYYY-MM-DD`, both inclusive (the same day boundary as `groupBy: ["day"]`).
They window rows, totals, `leadTouch` and `tasksBySource`; either end may be
left out. `sinceDays` works as before, but combining it with `from`/`to`, a
malformed date, or a `from` later than `to` is an error.

A new setting `dailyBudgetUsd` (default 0 = off) caps a UTC day's spend across
all repos and stages. Once today's spend reaches it, orchd does not start a new
plan run or implement attempt: the task waits with a `daily_budget` question
(options "run anyway" and "stop") that names today's spend, the budget, the
date and the run that was held. Attempts already running, and their review and
advisor runs, are never interrupted. "run anyway" lets that task run for the
rest of the UTC day (`dailyBudgetOkDay` on the task); "stop" stops it. The
answer policy and orchestrator triage never answer this question, and a
waiting task does not resume by itself when the day changes.
