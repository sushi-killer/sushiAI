## orchd: brief consistency check

- A cheap read-only model run (`briefCheckRoute`, default the new built-in
  route `claude-haiku`; empty turns it off) checks whether a task's goal and
  criteria contradict each other. It runs after the planner's draft (a
  contradiction gets one redraft per task) and before the first attempt of a
  task created with explicit criteria (the task keeps running, with a
  `Brief check: <conflict>` decision and a block in the implement and review
  briefs). Its cost counts toward the task.
