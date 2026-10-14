## Tasks wait for the owner less often, and shared checks are safer

- Only a check the planner wrote for its own test files, or for paths that do not exist on the base, can be dropped by the overseer, the answer policy or the judge. Every other check, whatever its runner (`pytest`, `go test`, `cargo nextest`, `just`, `gradle`, package scripts), is the owner's to drop. Tasks saved before this change keep every check owner-only.
- A planner question that is not blocking and has a recommended option goes to triage; when triage settles it, it becomes an assumption you can overturn, otherwise you are asked. `batchQuestions` is on by default. A plan with no verify command takes the first command the repository offers as an assumption when it runs the whole suite, and asks otherwise.
- A base carry that fails for a reason that can pass (a lock, a git error) is retried after the next fetch up to 3 times, then the agent gets it as a conflict to fix, instead of an owner question.
- A task waiting for an agent CLI or for the daily budget goes on by itself once a CLI can be started or the UTC day rolls over.
- A refused write into a temp directory is recorded and the task goes on. `WebFetch` and `WebSearch` follow `allowedDomains` (`*` allows every host without asking) and are denied outside a listed set, with no question. A criterion that needs a tool that cannot be enabled continues without it. Enabling a connected tool, writes to protected paths and outside services stay with you.
- An unknown cost stage is recorded under `other` instead of aborting the task loop.
