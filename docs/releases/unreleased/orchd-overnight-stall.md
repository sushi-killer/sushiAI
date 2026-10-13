## Orchestrator

- A base check that fails on a default branch that landing may not move (`landOnDefault` off for the repo) no longer waits for that branch to move. Before, the task waited forever, because only the owner's merge moves that branch.
- A task run's file edits inside its own worktree are allowed by orchd's edit hook. Before, headless Claude Code on a Linux host could refuse them, and each refusal became an owner-only permission question that parked the task.
- A Claude run that ends on `Failed to authenticate` or an overloaded API is paused with the rate-limit backoff and does not count as a failed attempt.
- When auto-answer is turned on, the overseer is woken once for each question that parked while it was off.

## App

- The host wait and the companion owner card keep their timeouts on Node 22, so the test suite no longer stops partway on a Node 22 host.
