## Orchestrator: start a task from a branch other than the current one

- The **+ New task** form gets a second, optional **Base branch** field. Type
  a branch name and the task starts from it instead of the checked-out
  branch; leave it blank (the placeholder says so) and orchd falls back to
  HEAD, same as before.
- A task's detail header now shows `from <baseRef>` next to its own branch
  whenever it was started from something other than the default - older
  tasks, and any task started the usual way, show nothing extra.
