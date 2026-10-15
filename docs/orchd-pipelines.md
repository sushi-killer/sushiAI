# Orchestrator pipelines

The executable pipeline definitions live in `crates/sushiai-orch/pipelines/`.
They are embedded in the daemon binary and decoded into private Rust types.
Changing a definition requires rebuilding the daemon.

| Definition      | Coordinator                               | Responsibility                                                                                              |
| --------------- | ----------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `task.yaml`     | `engine/lifecycle/run/cycle.rs`           | Execute, settle, inspect, qualify, review and complete one attempt; route retry, stop and failure outcomes. |
| `planning.yaml` | `engine/planning/run.rs`                  | Draft, review, answer questions, split, check commands and finish; route clarification and replanning.      |
| `delivery.yaml` | `engine/lifecycle/completion/delivery.rs` | Admit delivery, land and settle its result, or commit the task branch.                                      |
| `catalog.yaml`  | `engine/pipeline/stages.rs`               | Existing main/side stage IDs and labels returned by `pipeline.describe`.                                    |

The catalog preserves the existing UI grouping. An internal operation may contain
several visible stages: task qualification retains carry, evidence, verification,
evidence validation and review admission together in their Rust owner.

Each executable definition has an entry operation and typed transitions:

```yaml
entry: execute
transitions:
  - { from: execute, on: ready, to: settle }
  - { from: execute, on: retry, to: retry }
  - { from: execute, on: finished, to: finish }
```

The complete definition also names every outcome of the remaining operations.
`policy/pipelines/` validates operation names, allowed outcomes, unique and
complete transitions, and compatibility between the produced receipt and its
consumer. Coordinators read those transitions and dispatch the existing Rust
operations. Actual native, review, plan and landing receipts carry their results
between operations. No new persisted stage cursor is introduced.

Permissions, protected files, owner decisions, budgets, cancellation, snapshot
admission, accounting and publication stay inside their existing Rust operations.
A transition cannot manufacture a verified receipt or turn a stopped result into
a commit. Definition tests reject missing gates and incompatible edges. Runtime
coordinator tests exercise accepted owner edits and stopped tasks as well as the
normal paths.

The definitions contain operation and outcome IDs. They have no shell command,
expression, provider payload, dynamic import or runtime override fields. A new
operation needs a typed Rust binding and tests for its inputs and outcomes.
Existing presets continue to control routes and optional behavior through their
current settings contract.

For a change, run the targeted flow and coordinator tests, then `npm run ci`.
The full pipeline, RPC, recovery, cancellation, planning and landing suites remain
the behavior proof. The source comparison and final delivery evidence are linked
from [goal status](orchd-simplify-status.md).

The decoder uses the deserialization feature of
[serde-saphyr](https://docs.rs/serde-saphyr/latest/serde_saphyr/); the dependency
version and checksums are pinned in the workspace lockfile.
