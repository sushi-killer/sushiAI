## Orchestrator

- Running graphs pick up base fixes outside their held paths before each part's next attempt; `orch.task.sync` merges on demand and returns conflicting parent files after aborting the merge. A conflicting part carry gets one attempt whose brief names the files.
- `orch.task.park` quietly pauses a task with a visible queue reason, releases its file leases, and resumes on `task.start` or when its optional `until` task finishes. Dependents keep waiting without a dependency question.
- Explicit stops clear open questions and release leases immediately, including while a loop winds down. Agents and policy cannot restart explicitly stopped dependencies.
- Answers record their real source, including `source: "lead"` for raw RPC leads. After three retries of the same failure from any source, the next failure parks for a fix without asking again.
