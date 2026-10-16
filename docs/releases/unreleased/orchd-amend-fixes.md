## Amending and dropping criteria works on any task

- Dropping a criterion removes every copy of its command from the task's checks; a shared repo or owner command is kept unless the owner drops it.
- `task.amend` works on a failed task, and amended criteria rebuild their checks from their `-- check:` commands.
- Every plan brief tells the planner that a benchmark or measurement is a check orchd runs, not one the agent runs in its sandbox.
