## Orchestrator

- A Brainstorm or Plan proposal of two or more tasks is now one feature: a parent task whose branch every part branches from and lands on, so the whole feature ships as one branch and one PR. Rows created later from the same proposal join that feature. A one-task proposal still becomes a task of its own.
- Parts of a feature parked in the backlog wait with it and start only once the feature leaves the backlog.
- The orchestrator asks fewer questions: it asks only what only you can decide and settles the rest with a default it names in one line.
- The orchestrator's MCP server can plan the backlog: `task_create` takes `backlog`, and the new `task_backlog` tool moves a task between Next, Later and out. Any Claude Code session with the `sushiai-orchestrator` server attached can list, create and park tasks.
- The orchestrator no longer turns git-only work (cherry-picks, creating a branch, running CI) into a task that can only fail with "No files changed"; it gives you the command instead.
