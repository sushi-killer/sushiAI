## Orchestrator

- With auto answer on, the orchestrator agent wakes when a task waits on a question it can answer, answers it from the task and repository context, and otherwise writes you one question with options in its chat. Questions only you can answer (budget, permissions, protected paths) still go to you directly.
- The orchestrator agent opens a pull request when a task is done and green. It never merges.
- An answer from the orchestrator agent never counts as yours: it cannot accept an attempt that skipped review.
- New `digestHour` setting (JSON settings editor): once a day at that local hour the orchestrator writes a short digest in each active repository's chat — what finished, what waits on you, and today's spend.
- New Log view in the Orchestrator panel: what the orchestrator agent did across all projects (woke, answered, opened a pull request, left you a note, digest, skipped, failed), newest first, with a project filter and today's spend against the daily budget.
- A wake the orchestrator agent missed because its chat was busy is kept and runs when that turn ends. After a restart it is woken once per project that has a waiting question it may answer.
- New `waitRemindEveryHours` and `waitDefaultAfterHours` settings (JSON settings editor): remind the orchestrator agent every N hours about a waiting question, and after M hours answer a plan or agent question with its cautious option, recorded as an assumption you can overturn. Both are off by default.
- A usage or rate limit no longer fails an attempt: the task pauses (5, 15, then 45 minutes, or until the reset time the message states, at most six hours), shows the pause in its decisions and log, and resumes by itself. Interrupted attempts and harness errors that changed no file no longer count toward `maxAttempts`.
- The loop detector counts failing tool results only when the same error repeats, so a normal red-green test cycle is no longer stopped as a loop.
- A stopped task records who stopped it and why (`Owner: stop (UI)`), and `task.stop` takes an optional `reason`.
- The orchestrator agent no longer has the `task_stop` tool; its instructions already forbade using it.
