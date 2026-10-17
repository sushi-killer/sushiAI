## Orchestrator plans only checks the repository defines

- The planner proposes only scripts the repository defines today.
- When the repository's CI script already picks the checks a change needs, the planner uses it as the final check and adds no whole suites it would skip.
