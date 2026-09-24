## Orchestrator: archive tasks instead of deleting them

- The Orchestrator panel's task detail view gets an **Archive** action next to delete: it hides a task from the main list without touching its worktree, branch or record. A running, drafting or waiting task can't be archived - stop it first.
- A new **Archive** row in the panel's navigation lists every archived task with a one-click restore back to the main list.
- Archived tasks never resume automatically: a restart skips starting their loops, and `task.start` on an archived task asks you to unarchive it first.
