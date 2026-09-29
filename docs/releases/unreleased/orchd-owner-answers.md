## orchd: owner answers stick

- Answering "drop this check" or "retry after the base is fixed" to a check that already fails on the base is no longer asked again, including after a daemon restart, and an option answer is accepted whatever its case or trailing punctuation.
- orchd no longer asks for a screenshot on a criterion checked by a command (cargo, npm, pytest, ...), even when the planner marked it visual; task.amend accepts criteria as {text, visual} and replaces the visual flags with the amended ones.
- When only the review or the screenshot evidence failed, the "attempts keep failing" question now offers "accept the last attempt as done": picking it (or saying so in your own words) commits that attempt, after its final checks, instead of starting another one.
