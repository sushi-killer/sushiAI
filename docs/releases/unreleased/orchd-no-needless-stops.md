## Tasks wait for the owner less often

- A planner question that is not blocking and has a recommended option becomes an assumption you can overturn; `batchQuestions` is on by default. A plan with no verify command takes the first command the repository offers (a package script, `cargo test`, `go test`, `pytest`, `make test`) as an assumption, and asks only when it finds none.
- A blocked agent report now lists its answers and marks the safest one `(cautious)`, so `waitDefaultAfterHours` can apply it.
- A base carry that fails is retried after the next fetch up to 3 times, then the agent gets it as a conflict to fix, instead of an owner question.
- A task waiting for an agent CLI or for the daily budget goes on by itself once a CLI can be started or the UTC day rolls over.
- A refused write into a temp directory is recorded and the task goes on. `WebFetch` and `WebSearch` follow `allowedDomains` and are denied outside it, with no question. A criterion that needs a tool that cannot be enabled continues without it. Enabling a connected tool, writes to the repository's protected paths and outside services stay with you.
- A word such as `permission` or `token` inside quoted failure output or an impossible claim's evidence no longer keeps the overseer from answering.
- An unknown cost stage is recorded under `other` instead of aborting the task loop.
