## Orchestrator

- With auto answer on, the orchestrator agent wakes when a task waits on a question it can answer, answers it from the task and repository context, and otherwise writes you one question with options in its chat. Questions only you can answer (budget, permissions, protected paths) still go to you directly.
- The orchestrator agent opens a pull request when a task is done and green. It never merges.
- An answer from the orchestrator agent never counts as yours: it cannot accept an attempt that skipped review.
- New `digestHour` setting (JSON settings editor): once a day at that local hour the orchestrator writes a short digest in each active repository's chat — what finished, what waits on you, and today's spend.
