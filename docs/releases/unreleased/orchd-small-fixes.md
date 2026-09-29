## orchd: interrupted checks, moot questions, base-commit checks

- A final check or verify command killed by an orchd shutdown is now recorded as interrupted and re-run by the next orchd, instead of counting as a failed attempt and asking the owner.
- On start orchd clears a dependency question that became moot while it was down, without waiting for another task event.
- Every check and verify command orchd runs gets `ORCHD_BASE_SHA` (the task's base commit), and the planner is told to compare against it rather than a branch name and to write checks that fail before the work.
