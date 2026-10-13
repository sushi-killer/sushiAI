## Orchestrator

- A repo can add a `format` command to `.sushiai/setup.json`. orchd runs it over the planner's test files before it commits them, so protected tests already pass the repo's own format check and a task no longer stalls on a formatter it cannot run on them. A failing command commits the tests as written and records a decision.
