---
kind: report
title: Orch architecture comparison against d177
---

# Orch architecture comparison against d177

[Portable metrics and dependency pairs](orchd-simplify-metrics.json) accompany
this report. The full raw inventory files named below remain local proof
artifacts and can be regenerated with the checker.

**Verdict: the architecture objective is incomplete.** The final configured inventory reports **209 violations** and **zero exceptions**. Runtime behavior checkpoints passed, but the strict architecture gate cannot be called green. Moving an old file does not prove that its responsibilities or remaining long functions meet the standards.

## Comparable measurement

The baseline is the actual `d177c71d63f0969480c8328a9ffdc09113cecf40` source tree, verified with `git diff --quiet` and its raw-input digest. Both sides use the same final AST classifier: 32 synthetic tests and strict Clippy passed; the helper bytes in MAIN equal the tested isolated helper bytes. Main sources were measured at 2026-10-15T04:09:17+00:00 after the late P1 planning-publication, cancellation/lock, dirty-landing, lifecycle carry/admission, locked healing-publication and post-healing Retry receipt fixes were frozen and QA corrected the grounded-schema and flattened relay-schema fixtures. Raw digests were recomputed from the measured files and Cargo manifest and match all three stored reports.

These are physical lines **after rustfmt**, classified through AST scopes, including nested/interleaved `cfg(test)`. Runtime-classified lines include imports, declarations, comments and blank lines; they are not executable statement counts. Test/helper function counts are not counts of independently executable test cases. Scope is this crate, its referenced shared `../process.rs`, prompts and fixture data, rather than the whole workspace.

| Metric                                     | Before: d177 default | After: final default |   Delta |
| ------------------------------------------ | -------------------: | -------------------: | ------: |
| Scanned files                              |                  264 |                  701 |    +437 |
| Rust files                                 |                  186 |                  617 |    +431 |
| Runtime-classified physical lines          |               55,066 |               63,596 |  +8,530 |
| Test-classified physical lines             |               63,680 |               69,601 |  +5,921 |
| Data lines                                 |                2,550 |                2,728 |    +178 |
| Total classified lines                     |              121,296 |              135,925 | +14,629 |
| Production functions                       |                1,711 |                2,090 |    +379 |
| Test/helper functions                      |                2,485 |                2,601 |    +116 |
| Production functions >100 lines            |                   56 |                   28 |     -28 |
| Production functions >5 arguments          |                   84 |                   57 |     -27 |
| Dependency reference sites                 |                3,380 |               13,365 |  +9,985 |
| Unique cross-top-level directed edges      |                   73 |                  101 |     +28 |
| Unique top-level directed edges incl. self |                   86 |                  117 |     +31 |
| Production glob violations                 |                   97 |                    0 |     -97 |
| Rust files >=1,000 total lines             |                   32 |                   13 |     -19 |
| Mixed/runtime files >=1,000 total lines    |                   27 |                    8 |     -19 |
| Files >=1,000 runtime-classified lines     |                   15 |                    1 |     -14 |
| Default structural violations              |                  328 |                  130 |    -198 |

Runtime-classified source grew by **8,530 lines** and test-classified source by **5,921 lines**. This is not a net source-size reduction. Explicit imports replace 97 production glob sites, operations and private-state APIs are now declared at their owners, and extracted phases/serde ownership/tests have their own declarations. Existing tests moved out of large files without disappearing. Additional amendment, stale-evidence, backlog and native continuation regressions also add source. The PR and base/verification decompositions retain their behavior; their moved code must not be credited as deleted runtime work. No exact line-by-line causal attribution of the total growth is claimed.

### Dependency counts

A dependency reference site is one unique `(source module, resolved target path, source file, line)` recorded by the AST visitor. A single logical dependency can appear at an explicit import and at many type/call sites. Top-level directed edges collapse both ends to the first module segment, use `<crate>` for composition, and exclude self edges in the cross-top-level count. The full pair lists and added/removed sets are in `orch-final-metrics.json`.

Reference sites increased **3,380 → 13,365**, while cross-top-level directed edges increased **73 → 101**. The reference increase is inflated by explicit-import relocation and newly visible names: the old glob imports did not enumerate their imported symbols for resolution; the new owner files do. New first-level owners (`task`, `policy`, `api`, `verify`, `delivery`) also replace old `model`/`brief`/`mcp`/`chat` connections. Neither count measures runtime calls or proves reduced coupling. Genuine forbidden owner edges remain in the configured report and are not excused by this explanation.

### Default comparison versus configured gate

The comparable table uses default mode on **both** snapshots. Configured final mode adds `evals/set-1.json` and `evals/set-2.json` through the crate's `dataRoots`; all shared file metrics and all Rust file sets match. That intentional extra data scope gives configured mode a distinct input digest, not a source-race explanation. Configured dependency destinations also collapse to configured owner prefixes, so its reference count must not replace the default count in a before/after comparison.

| Final mode                 | Runtime |  Tests |  Data | Structural/boundary violations |
| -------------------------- | ------: | -----: | ----: | -----------------------------: |
| Default comparison         |  63,596 | 69,601 | 2,728 |                            130 |
| Configured gate diagnostic |  63,596 | 69,601 | 2,794 |                            209 |

The ignored report facilities deliberately exit successfully after emitting their findings. That is successful measurement, not a passed `workspace_standards` gate. The normal CI gate remains unconditional.

**Actual strict execution:** The compiled MAIN checker executed `workspace_standards`, which exited **101** with **209 violations**. Every emitted violation matches the configured inventory exactly, including multiplicity. The source input digests were revalidated after execution at 2026-10-15T04:09:19+00:00. This is an executed failing gate, not a compile failure or an inferred result; no configuration exemption was added.

## Every original Rust file with at least 1,000 total lines

`R/T` means runtime/test classified lines. “Old path removed” means the path is absent from the measured working tree; only the explicit deletions later in this report represent removed behavior. Each owner path below exists. Retained paths and the oversized relocated lifecycle driver are called out rather than counted as completed deletions.

| Original path                   |  Before R/T | Path/outcome                                   | Actual current owner(s) and limits                                                                                                                                                                                                                |
| ------------------------------- | ----------: | ---------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/brief.rs`                  | 2,411/2,044 | Old path removed; **Moved**                    | `src/engine/attempts/`; `src/engine/planning/`; `src/verify/review/`; `src/delivery/brief.rs`. Executor, planning, review and delivery briefing now live with their consumers; YAML was already upstream in d177.                                 |
| `src/engine/task_loop.rs`       |   4,058/195 | Old path removed; **Incomplete move**          | `src/engine/lifecycle/`; `src/task/loop_operations.rs`. Lifecycle phases moved, but lifecycle/run.rs still holds a 2,102-line driver in a 2,222-line runtime file.                                                                                |
| `src/model.rs`                  |   3,244/933 | Old path removed; **Moved**                    | `src/task/`; `src/policy/`; `src/api/`; `src/verify/`; `src/clock.rs`. Types moved to owners. Task has private data and read-only Deref; serde-transparent wrapper preserves the external data representation. This is no actor/schema migration. |
| `src/chat.rs`                   | 2,539/1,217 | Old path removed; **Moved**                    | `src/engine/chat/`. Chat sessions, launch/stream, proposals, reply and tools split; chat persistence currently remains engine/chat/store.rs.                                                                                                      |
| `src/git.rs`                    |   1,710/958 | Old path removed; **Moved**                    | `src/git/`; `src/delivery/git/`. Low-level repository operations stay in git; commit/carry/recovery scenarios have delivery owners. Remaining configured edges are still reported.                                                                |
| `src/engine/rpc_tasks.rs`       |   1,643/864 | Old path removed; **Moved**                    | `src/engine/rpc/`; `src/api/dispatch.rs`; `src/task/creation.rs`. Wire dispatch/validation reaches typed task command services; create/amend/control/answer/graph operations are separate.                                                        |
| `tests/graph.rs`                |     0/2,127 | Retained; **Unresolved test file**             | `tests/graph.rs`. Retained unchanged at 2,127 test lines; no test-size exception.                                                                                                                                                                 |
| `src/engine/base_check.rs`      |   1,379/627 | Old path removed; **Moved**                    | `src/engine/verification/base/`; `src/verify/base_cache.rs`; `src/task/amendment.rs`. Repository/base jobs and answer orchestration separated from cached evidence and pure amendment application.                                                |
| `src/engine/verify.rs`          |   1,419/493 | Old path removed; **Moved**                    | `src/verify/`; `src/engine/verification/`. Command execution/evidence/comparison/scoping separated from task-aware cache, baseline and held-out orchestration.                                                                                    |
| `src/engine/graph.rs`           |    1,832/54 | Old path removed; **Moved**                    | `src/engine/graph/`; `src/task/graph.rs`. Parent scheduling, landing, dependencies, relay and carry separated from pure graph queries.                                                                                                            |
| `src/harness.rs`                |     996/880 | Old path removed; **Moved**                    | `src/harness/`. Provider argv/settings, streamed outcome/session parsing, fingerprint, native process and guard capabilities have canonical owners.                                                                                               |
| `src/engine/plan.rs`            |   1,729/105 | Old path removed; **Moved**                    | `src/engine/planning/`; `src/task/planning/`; `src/task/checks/plan.rs`. Planning run/round/correction/split/records/validation separated; Task applies the proposed plan.                                                                        |
| `src/engine/permissions.rs`     |   1,221/574 | Old path removed; **Moved**                    | `src/engine/permissions/`; `src/policy/permissions.rs`; `src/store/permissions.rs`. Task tool/hook and answer coordination, pure permission rules and grant-file IO separated.                                                                    |
| `src/engine/attempt_run.rs`     |   1,174/372 | Old path removed; **Moved**                    | `src/harness/native/`; `src/harness/guard.rs`; `src/engine/attempts/`. Launch/stdio/watch/process state separated from attempt observation, redaction configuration, pricing and single settlement. Operation argument audits remain.             |
| `src/engine/healing.rs`         |   1,114/431 | Old path removed; **Moved**                    | `src/engine/verification/healing/`; `src/verify/healing/`; `src/task/checks/edits.rs`. Preflight/answers/evidence orchestration, reply/scope parsing and pure contract edits separated.                                                           |
| `src/engine/questions.rs`       |     774/702 | Old path removed; **Moved**                    | `src/engine/questions/`; `src/task/questions/`. Reply recording, parking, answers/holds/wake and exhausted handling separated; Task owns question state.                                                                                          |
| `src/engine/answer_policy.rs`   |     864/602 | Retained; **Unresolved mixed file**            | `src/engine/answer_policy.rs`; `src/task/checks/authority.rs`; `src/task/checks/edits.rs`. Duplicate direct mutations delegate to Task authority/edit operations, but answer policy remains 681 runtime +659 test lines.                          |
| `src/store.rs`                  |     774/659 | Old path removed; **Moved**                    | `src/store/`; `src/engine/recovery/interrupted.rs`; `src/task/migrations/`. JSON/settings/cost IO split; interruption coordination and pure legacy state migrations moved to their owners. Derived precedent sync stays at save IO.               |
| `src/mcp.rs`                    |     824/563 | Old path removed; **Moved with function debt** | `src/api/mcp/`. Schema, session, scope and call handling split. schema::tool_specs remains a 330-line function; no exception.                                                                                                                     |
| `tests/review.rs`               |     0/1,339 | Retained; **Unresolved test file**             | `tests/review.rs`. Retained unchanged at 1,339 test lines; no test-size exception.                                                                                                                                                                |
| `src/engine/wait_sla.rs`        |   261/1,050 | Old path removed; **Moved**                    | `src/engine/wait_sla/`; `src/policy/waits.rs`. Tick/resume/restart/timer orchestration split from pure timing predicates; retained tests extracted into cohesive files.                                                                           |
| `src/engine/protected.rs`       |     612/660 | Retained; **Unresolved mixed file**            | `src/engine/protected.rs`; `src/task/checks/edits.rs`. File/protection adapter remains 651 runtime +701 test lines; pure planner adoption now belongs to Task.                                                                                    |
| `src/engine/pr_draft.rs`        |   1,070/181 | Old path removed; **Moved**                    | `src/engine/delivery/pr/`; `src/delivery/pr/`; `src/delivery/git/`. Prepare/draft/open/publish/pending/retry/carry coordination and pure rendering/repository delivery separated. This is relocation, not PR behavior deletion.                   |
| `src/evolution/propose.rs`      |     769/448 | Retained; **Unresolved mixed file**            | `src/evolution/propose.rs`; `src/evolution/proposal.rs`; `src/policy/evolution.rs`. Types/policy moved, proposal orchestration remains 807 runtime +452 test lines.                                                                               |
| `src/engine/decision_memory.rs` |     427/756 | Retained; **Unresolved mixed file**            | `src/engine/decision_memory.rs`; `src/task/notes.rs`. Task state writes now use owner operations; file remains 442 runtime +843 test lines.                                                                                                       |
| `src/engine/tests/verify.rs`    |     0/1,172 | Retained; **Unresolved test file**             | `src/engine/tests/verify.rs`. Retained and adapted to canonical Task/verification owners: 1,206 test lines.                                                                                                                                       |
| `src/engine/mod.rs`             |    1,092/74 | Retained; **Moved**                            | `src/engine/mod.rs`; `src/engine/state.rs`; `src/engine/runtime.rs`; `src/engine/run_control.rs`; `src/api/`. Entry is now 91 runtime +5 test lines; App/synchronization/runtime and host wire types have explicit owners.                        |
| `src/evolution/detect.rs`       |     579/537 | Retained; **Unresolved mixed file**            | `src/evolution/detect.rs`; `src/task/evolution.rs`. Task-facing types moved; detection implementation and inline tests remain 583 runtime +554 test lines.                                                                                        |
| `src/ab.rs`                     |     596/502 | Retained; **Unresolved mixed file**            | `src/ab.rs`. AB evaluation/report implementation retained at 597 runtime +502 test lines.                                                                                                                                                         |
| `tests/pr_on_done.rs`           |     0/1,088 | Retained; **Unresolved test file**             | `tests/pr_on_done.rs`. Retained unchanged at 1,088 test lines; no test-size exception.                                                                                                                                                            |
| `src/engine/review.rs`          |     963/108 | Retained; **Unresolved mixed file**            | `src/engine/review.rs`; `src/verify/review/`. Brief/reply/routing responsibilities split, review service remains 956 runtime +112 test lines.                                                                                                     |
| `tests/task_loop.rs`            |     0/1,062 | Retained; **Unresolved test file**             | `tests/task_loop.rs`. Retained with resume/current-contract regression coverage: 1,132 test lines; no test-size exception.                                                                                                                        |

## Every remaining current Rust file with at least 1,000 total lines

All 13 entries below remain unresolved and have no exceptions. 8 contain runtime and are mixed/runtime files subject to the mandatory split rule; 5 are full test files. Only the lifecycle driver has at least 1,000 runtime-classified lines by itself. A mixed file can violate the mandatory 1,000-total-line rule even when its separate runtime count is below 600.

| Current file                    | Runtime | Tests | Total | Unresolved responsibility                                                                                                     |
| ------------------------------- | ------: | ----: | ----: | ----------------------------------------------------------------------------------------------------------------------------- |
| `src/engine/lifecycle/run.rs`   |   2,222 |     0 | 2,222 | Orchestration driver still spans 2,102 function lines; extracting surrounding phases did not complete this split.             |
| `tests/graph.rs`                |       0 | 2,127 | 2,127 | Retained unchanged at 2,127 test lines; no test-size exception.                                                               |
| `src/engine/protected.rs`       |     651 |   701 | 1,352 | File/protection adapter remains 651 runtime +701 test lines; pure planner adoption now belongs to Task.                       |
| `src/engine/answer_policy.rs`   |     681 |   659 | 1,340 | Duplicate direct mutations delegate to Task authority/edit operations, but answer policy remains 681 runtime +659 test lines. |
| `tests/review.rs`               |       0 | 1,339 | 1,339 | Retained unchanged at 1,339 test lines; no test-size exception.                                                               |
| `src/engine/decision_memory.rs` |     442 |   843 | 1,285 | Task state writes now use owner operations; file remains 442 runtime +843 test lines.                                         |
| `src/evolution/propose.rs`      |     807 |   452 | 1,259 | Types/policy moved, proposal orchestration remains 807 runtime +452 test lines.                                               |
| `src/engine/tests/verify.rs`    |       0 | 1,206 | 1,206 | Retained and adapted to canonical Task/verification owners: 1,206 test lines.                                                 |
| `src/evolution/detect.rs`       |     583 |   554 | 1,137 | Task-facing types moved; detection implementation and inline tests remain 583 runtime +554 test lines.                        |
| `tests/task_loop.rs`            |       0 | 1,132 | 1,132 | Retained with resume/current-contract regression coverage: 1,132 test lines; no test-size exception.                          |
| `src/ab.rs`                     |     597 |   502 | 1,099 | AB evaluation/report implementation retained at 597 runtime +502 test lines.                                                  |
| `tests/pr_on_done.rs`           |       0 | 1,088 | 1,088 | Retained unchanged at 1,088 test lines; no test-size exception.                                                               |
| `src/engine/review.rs`          |     956 |   112 | 1,068 | Brief/reply/routing responsibilities split, review service remains 956 runtime +112 test lines.                               |

## Actual runtime deletions and mutation consolidation

| Change                                                         | Evidence in final source                                                                                                                                                                                                         | What is preserved / what is not claimed                                                                                                                                                                                                                                                                                                       |
| -------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ERROR_STREAK` rule removed                                    | `loop_detect.rs` no longer defines the constant, `error_run`/`last_error`, result-text normalization or the `error_streak` verdict. Its classification changed from 214 runtime/156 test lines to 136/109.                       | Exact repeated-call and repeated-edit guards remain. Stall, budget, cancellation and attempt gates remain. The 78-runtime-line net reduction in this file is not attributed to every other relocation.                                                                                                                                        |
| Extra timeout report LLM turn removed                          | `engine/time_budget.rs` has no `run_harness`, `REPORT_TURN_MINS` or report-only request. `time_budget_report_prompt` and its golden file are absent. It is now a 44-runtime-line parsed-handoff/fallback operation (112 before). | Existing summary/handoff and changed-file fallback remain. Native early-session capture and existing Verify/TimeBudget continuation are used; this is not a second resume mechanism. No report run or report cost is added.                                                                                                                   |
| Duplicate criteria/check mutation implementations consolidated | Private `Task.data` and read-only `Deref` require business state writes to pass through task operations. `task/checks/{criteria,authority,edits,plan,baseline,snapshot}` and `task/amendment.rs` own edits and authority.        | Some engine adapter function names remain: e.g. `decision::drop_criterion` captures the operation and delegates, and `answer_policy::remove_check_command` resolves repository authority then delegates. The old duplicate in-place mutation bodies are gone; these adapters are not claimed as deleted files or as eliminated orchestration. |
| PR/native/serde decomposition                                  | Canonical delivery, native process and owner type paths replace old large files.                                                                                                                                                 | This is preserved behavior and ownership relocation, not deletion of PR publication, process control, checks or persisted JSON support. No actor, revision/generation or provider protocol migration is claimed.                                                                                                                              |

The baseline already contains merged upstream YAML/terminal work. This comparison does not credit their existing code as a new deletion or a new feature in this change.

## Remaining configured violations

| Category           | Count |
| ------------------ | ----: |
| arguments          |    63 |
| entryFiles         |     2 |
| functions          |    38 |
| mixedRuntime1000   |     8 |
| moduleEdges        |    78 |
| productionFiles    |     8 |
| prohibitedReexport |     1 |
| testFiles          |    11 |

There are 78 configured forbidden module-edge reports, 63 argument-count reports and 38 function-size reports (the latter two include tests as well as production), along with the file/entry/reexport findings listed above. No forbidden pure-module reference was emitted by this configured snapshot. This static result is useful evidence for Task/policy separation; it does not clear the other boundary or size failures. No broad or narrow exceptions were registered to hide these findings.

## Proof and reproducibility

The generic checker proof remains **32/32 synthetic tests passed**, with `cargo clippy -p sushiai --test rust_standards -- -D warnings` passing on the identical tested helper bytes. It covers threshold boundaries, interleaved tests, malformed/duplicate configuration, custom Cargo targets, exact shared sources/exceptions, aliases and local modules, canonical reexports, macro labels/members and real forbidden nested paths, plus absolute external paths. The compiled nested Task-style reexport fixture passed rustc.

Runtime evidence from the implementation checkpoint: **9/9 attempt-budget tests**, **4/4 best-of integration tests**, and **15/15 extracted native/best-of unit tests** passed. Integration tests used fake harnesses, temporary homes/sockets and a copied test binary after sandbox socket denial. The subsequent final backlog regression passed against current code and failed against only the old pre-fix start body, proving that the owner contract had been overwritten there. These checkpoint results do not mean full final CI or the strict architecture gate passed.

Reproduce the default inventory and the unconditional configured gate from the workspace:

```sh
RUSTC_WRAPPER= RUST_STANDARDS_REPORT_ROOT="$PWD/crates/sushiai-orch" RUST_STANDARDS_REPORT_OUTPUT="$PWD/artifacts/orch-final-default.json" cargo test -p sushiai --test rust_standards --offline standards_report -- --ignored --exact
RUSTC_WRAPPER= cargo test -p sushiai --test rust_standards --offline workspace_standards -- --exact
```

The configured JSON was captured through the lead's isolated diagnostic: parse the crate configuration with `Config::parse`, call the identical `checker::analyze(root, Some(&config))`, and serialize the returned report. That private ignored diagnostic does not add a shipped fallback or weaken `workspace_standards`. The normal gate is the second command and fails while the listed violations remain. `orch-final-metrics.json` contains the full outcomes, dependency pair lists, exception count, report hashes and comparisons.

| Fingerprint                       | Value                                                              |
| --------------------------------- | ------------------------------------------------------------------ |
| Base commit                       | `d177c71d63f0969480c8328a9ffdc09113cecf40`                         |
| Classifier helpers SHA256         | `723e617340cc7147b4c0ddbcbc7036f01af0b6ce434b34781d7c86e92048e4c0` |
| Before raw input digest           | `4cb91baf8b1ddf029609a2c6734ce0a06084277406b7208b0919be24f5a56cdf` |
| Final default raw input digest    | `1784eb813c3754651e1fee49c4bdf8a58c3e74ac15c77697ffd638e8fa1c6d04` |
| Final configured raw input digest | `a9144d00d1686d55d801d7f6a5fd5a36255cb890f90face2c72662c478da3096` |
| Crate config SHA256               | `ed2b93d41a7fdd4d62b849463f9e2ff4917c26515ebc50cbf44ca00059cf2433` |

Files: `orch-before-d177.json` + `.meta.json`, `orch-final-default.json`, `orch-final-configured.json`, `orch-final-metrics.json`. All are proof artifacts, not source exceptions.
