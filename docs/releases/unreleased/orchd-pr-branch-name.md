## Pull request branches follow the repository's rules

- orchd now asks the drafting agent to name the pull request branch by the repository's own rules (AGENTS.md, CONTRIBUTING and the CI checks on branch names and titles), so the branch prefix pairs with the title when the repository requires it.
- A name that is invalid, is the base branch or already exists falls back to the task's own branch, and the decision is recorded on the task.
