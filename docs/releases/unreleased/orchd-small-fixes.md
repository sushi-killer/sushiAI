## orchd: interrupted checks, moot questions, base-commit checks

- A final check or verify command killed by an orchd shutdown is now recorded as interrupted and re-run by the next orchd, instead of counting as a failed attempt and asking the owner.
- On start orchd clears a dependency question that became moot while it was down, without waiting for another task event.
- Every check and verify command orchd runs gets `ORCHD_BASE_SHA` (the task's base commit), and the planner is told to compare against it rather than a branch name and to write checks that fail before the work.
- A review reply is now parsed leniently: a verdict is taken from the last `sushi-review` block, a `json` fence, an unlabelled fence or the last bare JSON object, an object closed too early before a trailing `"criteria"` is repaired, and a missing verdict is derived from the criteria. A reply with no usable verdict re-runs only the review, once per attempt, on the same diff (in `runs/<n>/review-2/`), instead of losing the attempt; only a second miss asks the owner, and `retry` there reviews the same attempt again. The no-verdict message quotes the start of the reply.
