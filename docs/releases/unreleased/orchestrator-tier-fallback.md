## Orchestrator: a task's tier fallback is now visible

- When orchd picks a task's first implement tier without a usable classifier
  answer or a planner tier, it now falls back to `standard` and records why:
  `classifier off`, `no classifier key`, `classifier call failed`,
  `no tier answer`, or `unsure: <choice> p <p>` - never the raw classifier
  error text, a key, a URL, or a response body. The task's `decisions` gain a
  `Jev: tier unavailable (<reason>) -> fallback standard, route <route>` line
  instead of the previous, misleading `Jev: tier hard (p 0.45) -> ...` line
  for a low-confidence answer.
- The task detail view's meta line now shows the tier (`standard tier`), and
  on a fallback the reason next to it in a warning tone (`standard tier
  (fallback: no classifier key)`).
