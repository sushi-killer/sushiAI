## Orchestrator compiler cache hits across worktrees again

- When a host cargo setup points every checkout at one shared target directory, orchd gives each task worktree its own target. The compiler cache now still hits across those worktrees; before, every worktree compiled from scratch.
