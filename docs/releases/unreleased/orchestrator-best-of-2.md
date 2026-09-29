## Orchestrator: best-of-2 on the hard tier

- The `bestOf` variant flag (2 = on) runs the first implement attempt of a hard task twice at once, in the task's worktree and a sibling `-b` one, on the tier route and on `bestOfRoute` (default: the first route of the other harness). Each candidate runs verify and the grounded checks; the one that passes wins, and when both pass the other-family reviewer reads both diffs and picks, its reason recorded as a decision line. When neither passes, the retry continues from the candidate with fewer failing checks.
- The winner's changes stay in the task worktree and the loser's worktree and branch are removed. Both candidates' costs count toward the task and its budget, and the attempt records `candidates`. Later attempts run once.
