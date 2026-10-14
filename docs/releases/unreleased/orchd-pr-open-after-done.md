## A done task's pull request opens after a carry onto a moved base

- The worktree that carries a done task onto the newest base now gets the repo's own setup before the checks run again, so the pull request opens. A setup failure there is reported as an environment problem with one notice, not as a work that could not be carried.
- The Open PR form starts with an empty title and branch, so orchd drafts both by the repository's rules; a value you type still wins.
