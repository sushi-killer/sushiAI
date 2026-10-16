## Orchestrator runs whole-suite checks once per task

- A package script such as `test:daemon` or `test:desktop` that starts a whole test suite, and `cargo nextest run` without a scope, now run once in a task's final checks instead of after every attempt.
- The planner proposes only scripts the repository defines today.
