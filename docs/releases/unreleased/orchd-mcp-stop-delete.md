## The orchestrator can stop and delete tasks

- The owner's orchestrator MCP has `task_stop` (ends a running, drafting, waiting or landing task) and `task_delete` (only for a task that is already stopped, and only when the owner asks). An overseer turn never deletes, and task-scoped or read-only bridges get neither tool.
