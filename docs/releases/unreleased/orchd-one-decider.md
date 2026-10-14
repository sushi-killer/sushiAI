## orchd asks the owner less, and only about the owner's calls

- One decider settles a waiting question: the overseer, when auto-answer and the answer policy allow it. Otherwise the question joins the owner's batch. The separate triage turn is gone.
- On "attempts keep failing" the overseer can only continue or change the approach. When nobody answers, the failure ladder ends the task with a report instead of leaving it waiting.
- The planner asks the owner only about a closed list of calls: scope, a public API or contract, money or an outside service, and destructive or security changes. Questions and reports tag what a question commits the owner to (`risk`), and a tagged answer is never taken by default.
- A default taken after the wait time never passes a hard limit, and a check that the project shares is never dropped by a plan review.
- Planner tests live in orchd's task store, not in a commit on the base. They are written over the worktree before checks and land in the final commit.
- The pull request title and body come from a schema-checked reply. A second turn checks that the body matches the diff before the push. orchd reads the pushed branch and the opened pull request back. Its own refusals are typed, so a retry never depends on error text.
- A declined review finding is ruled on by the next review, and a full review is briefed with the areas a cheap route turn picked.
