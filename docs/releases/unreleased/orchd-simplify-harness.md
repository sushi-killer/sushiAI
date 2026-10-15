## Orchestrator

- Accepted task amendments survive concurrent planning, starts, baseline checks and loop retirement. A stopped task also refuses a late PR carry result.
- A task that reaches its time budget can continue in the same agent session when its route is unchanged. Orchd keeps the handoff already received or records the changed files, without starting a separate report turn.
- A resumed session receives the current criteria and checks after an amendment. Previous failures are labelled as history so a removed check does not remain an instruction.
- Repeated red tool results no longer stop an agent that is trying different commands. Exact repeated calls and edits, stall detection, budgets, cancellation and attempt limits still apply.
