## Orchestrator

- A file lease now holds only the files a task has really changed, read from its worktree diff while it runs (also files a shell command wrote) and from `hook.edit`. A planned path never parks a task, so tasks whose plans overlap but whose edits do not run and land together. Planned paths still order sibling subtasks.
- A landing carry that rewrites a file the review saw sends the attempt back through verify and the review before it lands; a real conflict still gets exactly one more attempt on the newest base, with the files in its brief.
- The base preflight and the final base jobs start with the stored draft, beside the brief check, and start again when a redraft changes the commands.
- A waiting task shows what it waits for. The free-text `queueReason` is replaced by a structured `wait` (`slot`, `subtaskSlot`, `dependency`, `file`), and the panel shows one label for each kind.
