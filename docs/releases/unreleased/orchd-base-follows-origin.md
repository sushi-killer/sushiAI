## Orchestrator

- A task that waits for its base branch to move fetches that branch from `origin` once a minute and fast-forwards the local branch (clean checkout only), so a pull request merged on the remote wakes the waiting tasks on every host without a manual pull.
- The orchestrator agent, on a shared check that fails on a default branch only the owner moves, creates one task that fixes it, waits for the fix, and tells the owner once.
