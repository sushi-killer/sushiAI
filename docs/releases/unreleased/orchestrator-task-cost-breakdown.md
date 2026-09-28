## Orchestrator: a task's variant and cost breakdown, without opening task.json

- A task's detail view now shows a `Variant: ...` line under the header meta:
  `default` when it ran with the current experiment settings, `not recorded`
  for a task from before variants existed, or only the flags that differ
  (e.g. `retryMode fresh · leanOutput on`).
- Below it, a compact cost breakdown shows where the task's money went, by
  stage - Plan, Implement #n per attempt, Review, and Other (auto-answer
  runs and, for a task recorded before this change, reviews with no cost
  attributed to any attempt) - each shown only when it rounds above $0.00.
- orchd now records each review run's cost on the implement attempt it
  reviewed, as `reviewCostUsd`, alongside the existing `task.costUsd` total;
  a task recorded before this change simply has no `reviewCostUsd` and its
  review cost shows up under Other instead.
