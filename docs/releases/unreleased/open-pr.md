## Open a pull request from a task

- A finished Orchestrator task has an **Open PR** button: pick the pull request title and the remote branch name (for example `feature/x` instead of `task/x`), and orchd pushes the branch to `origin` and opens a GitHub pull request against the default branch with the goal, the criteria and the report. The task then shows **View PR**. Requires the `gh` CLI to be signed in.
