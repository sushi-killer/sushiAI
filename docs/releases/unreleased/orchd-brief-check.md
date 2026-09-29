## orchd: brief consistency check

- A cheap read-only model run (`briefCheckRoute`, default the mechanical
  tier's route, `codex`; empty turns it off) checks whether a task's goal and
  criteria contradict each other. A `briefCheckRoute` that names no
  configured route (such as the retired `claude-haiku`) falls back to the
  mechanical tier's route and records a decision line saying so. It runs
  after the planner's draft (a contradiction gets one redraft per task) and
  before the first attempt of a task created with explicit criteria (the task
  keeps running, with a `Brief check: <conflict>` decision and a block in the
  implement and review briefs). Its cost counts toward the task.
