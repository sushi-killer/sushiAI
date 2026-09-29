## orchd: gc prunes old evidence

- `orchd gc` (`worktrees.gc`) also removes `tasks/<id>/runs/*/evidence` of
  archived tasks last updated more than 14 days ago, and clears those
  attempts' `evidence` lists. The report has an `evidence` list of
  `{task, path, action: removed|would-remove, bytes}` and counts them in
  `freedBytes`; a dry run changes nothing.
