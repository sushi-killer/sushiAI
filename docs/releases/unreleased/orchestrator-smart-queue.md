# Orchestrator: tasks on one branch no longer collide

Tasks on the same base branch now take leases on the files they edit, so two
of them never work on one file at once and conflicts stop costing full
re-implement attempts.

- The planner lists the `paths` a task edits. A task whose paths overlap a live
  task's waits `queued`, with "waits for <task> on <path>" in its row and
  decisions, and starts by itself when that task lands, stops or fails. It
  first moves onto the new base head, so it edits on top of the landed work.
  Tasks with disjoint paths, or none declared, run in parallel as before.
- Claude runs check each edit (`Edit`, `Write`, `MultiEdit`, `NotebookEdit`)
  against the leases. An edit to a file another live task holds is refused with
  the holder's name, and the agent carries on with other files.
- Subtasks whose declared paths overlap run as a relay on one branch: each
  starts from the previous one's commit with a fresh agent and the previous
  handoff, and the parent lands the chain once. Disjoint subtasks stay parallel.
- A parent runs at most `childParallel` (default 3, in Settings) subtasks at
  once. A subtask is planned only when everything it depends on has landed, and
  stopping the parent stops the unplanned ones before any planning is spent.
- A landing that conflicts gets a conflict-only attempt on the cheap route
  (resolve the markers, run verify) instead of a full re-implement attempt.
