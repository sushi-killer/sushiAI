## Orchestrator

- Accepted task amendments survive concurrent planning, starts, baseline checks and loop retirement. A stopped task also refuses a late PR carry result.
- Cancelled runs retain usage and cost, pending amendments retain the actual timeout result, and a relay keeps earlier work when its base moves between attempts.
- Owner overrides of a check waiver remain active while checks, an agent turn or PR preparation finish.
- A task that reaches its time budget can continue in the same agent session when its route is unchanged. Orchd keeps the handoff already received or records the changed files, without starting a separate report turn.
- A resumed session receives the current criteria and checks after an amendment. Previous failures are labelled as history so a removed check does not remain an instruction.
- Repeated red tool results no longer stop an agent that is trying different commands. Exact repeated calls and edits, stall detection, budgets, cancellation and attempt limits still apply.
- Pending amendments are stored with the task and applied after a daemon restart. Amending a waiting task returns without waiting for its question or baseline check; Start clears an existing pause.
- Final checks after a PR carry rerun and compare failures with the base before blocking delivery. Persistent new failures give the task's own agent one repair step, then one owner question if the same cause remains.
- PR Validation is built from recorded commands, exits, checkout commits, checked trees and named waivers. The drafting agent describes the change and its purpose.
- A subtask's parent-landing conflict uses the Base moved brief with the conflicting files.
- Task execution, planning and delivery schemas now live in compiled YAML. Typed Rust operations retain their gates; the stage catalog keeps the existing pipeline view.
