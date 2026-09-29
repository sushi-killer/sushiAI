## Orchestrator: a task's tier fallback is now visible

- When a task has no planner tier, orchd runs its first implement attempt on
  `standard` and records why: `tierFallback` is `no planner tier`, and the
  task's `decisions` gain an
  `Orchestrator: no planner tier -> standard, route <route>` line.
- The task detail view's meta line now shows the tier (`standard tier`), and
  on a fallback the reason next to it in a warning tone (`standard tier
  (fallback: no planner tier)`).
