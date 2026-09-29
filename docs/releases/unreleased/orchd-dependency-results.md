## Orchestrator: a task sees what its dependencies delivered

- A task that depends on other tasks now gets their results in its brief:
  under `## Landed dependencies`, each finished dependency shows its title,
  the summary of its last passing implement attempt, its agent and owner
  decisions (the last five), its handoff and the files it changed (the first
  20, then a count of the rest).
- The section appears in the first implement brief and in the plan brief of a
  task whose dependency was already done. A resumed attempt gets only the
  dependencies that landed since its previous attempt began.
- Everything an agent wrote sits inside a data fence and each entry is capped
  at 1,500 characters. Tasks without dependencies are unchanged.
