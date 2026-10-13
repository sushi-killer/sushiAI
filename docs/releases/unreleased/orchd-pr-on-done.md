## Orchestrator

- A finished top-level task that cannot land because its base is the repo's default branch can now open a pull request by itself. Turn on `prOnDone` (or `prOnDoneRepos` for one repo): a read-only helper agent reads the repo's own rules (AGENTS.md, CONTRIBUTING.md, the pull request template) and drafts the branch name, title and body, then orchd pushes the branch and opens the pull request. The task stays done, and its pull request link shows in the task. If the draft fails, the pull request opens with the task's own title and branch. `prRoute` picks the drafting route.
