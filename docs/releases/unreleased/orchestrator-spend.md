## Orchestrator: spend by stage and model, per task and across tasks

- orchd now writes one cost record per model run to `<data>/costs.jsonl` when
  the run ends (an interrupted run is recorded with its estimated cost): plan,
  implement, review, advisor, pick, triage, audit, evolution proposal and the
  orchestrator chat, each with its route, the model that really ran, tokens
  (input, cached, output) and duration.
- `orchd costs --data <dir> [--since 7] [--by stage,model] [--json]` prints
  spend grouped by stage, model, route, repo, task or day, with totals and the
  cache hit rate. `orchd costs backfill --data <dir>` derives records from
  tasks, audits and proposals recorded before this change; the daemon also
  does it once on start, and never twice for the same task.
- The `costs.summary` RPC returns the same rows, and `task.get` adds
  `costByStage`.
- The orchestrator page shows a compact Spend block above the chat: the last 7
  or 30 days by stage and by model (top 5). A task's cost line now names every
  stage it spent on - review, advisor and the rest - instead of `Other`.
