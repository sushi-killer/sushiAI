## Failed tasks can wait for a fixing task

- `task.amend` with `dependsOn` now works on any task that is not implementing, including a failed task and a task that already ran implement attempts. Amending a failed task to depend on another task queues it behind that task, and it starts by itself once the dependency is done. `dependsOn: []` on a failed task leaves it failed. A task, or one of its children, that is implementing is still refused.
