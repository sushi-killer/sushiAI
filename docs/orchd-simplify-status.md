# Orchd simplification: draft checkpoint

This change is incomplete and must remain a draft. The strict Rust architecture
gate remains enabled and rejects the remaining oversized functions, mixed files,
and module dependencies. There are no registered size or dependency exceptions.
The implementation scope closed at 05:30 MSK on 15 October 2026; subsequent work
is limited to verification, review findings, documentation, and preserving this
checkpoint. Passing targeted regressions does not satisfy the architecture gate.

[The measured inventory](orchd-simplify-inventory.md) maps every original file
with at least 1,000 lines to its current owner and records all remaining large
files. Comparable post-rustfmt runtime lines grew from 55,066 to 63,596; test
lines grew from 63,680 to 69,601. The configured gate reports 209 violations
and zero exceptions. The inventory distinguishes actual removals from moves.

## Sources and integration

The integration base is `d177c71d63f0969480c8328a9ffdc09113cecf40`, including
merged #88, #89, and #90. The lab amendment task's settled final commit is
`c16c27bec9ea0e988ca84f2738aab76d655b44b3`. Only its delta after
`03c81608d60d82273aa193fa5169ec6fc2313ce7` was imported. The merged work was
not implemented again. No terminal performance change is included.

The full owner brief and research remain preserved separately, including all
twelve recommendations and the historical lab snapshot. That snapshot is not a
current performance benchmark. Recorded costs are provider estimates rather
than invoices; missing Codex cost values do not imply free execution. The old
ERROR_STREAK comment's 13 interruptions among 186 tasks is a different sample
from the research's 24 tasks and 155 retained implementation attempts.

## Implemented behavior

- `Task` owns production mutations through pure operations. Its persisted JSON
  shape and RPC names remain unchanged. Only test fixtures have direct mutable
  access. Planner, repository, owner, and unknown origins retain distinct
  authority; a textual mention of a command cannot remove its executable check.
- Captured positional edits carry their original content precondition. A replay
  cannot delete the neighboring criterion after indices shift. A new positional
  RPC is a new operation.
- Plan admission, pending amendment retirement, start admission, and delivery
  publication serialize against the same task lock. Fully parent-drafted
  children enter Queued after parent and dependency admission; ordinary drafts
  still require planning. Answer-derived edits publish before native admission.
  A stale answer retry retains the current best-of receipt within the same
  attempt; it does not adopt the primary raw result. Baseline IO runs outside
  standard mutex guards; its result must match the full current plan snapshot.
  Screenshot changes also invalidate that identity. Native planner results carry
  the input snapshot, apply accepted edits first, and discard stale proposals.
  Relay-base moves publish through the same guarded carry operation before
  native-attempt admission. Carry can re-anchor a
  pending edit only across a change of base with the same check contract.
- Final checks must authorize the current plan before delivery starts. A
  stopped or changed task refuses late carry publication, including manual PR
  retries. First queued carries have a separate gate from delivery admission.
  Restart recovery recognizes only a clean commit with matching task and
  attempt trailers. Entering delivery resets a previous dirty-checkout timer
  once; retries preserve that timer and record the first waiting reason.
- Red tool results no longer trigger ERROR_STREAK. Exact repeated calls and
  edits, stalls, cancellation, budgets, and attempt caps remain enforced.
- Time-budget expiry preserves the received report or a deterministic handoff;
  it no longer starts a report-only model turn. The existing early native
  session parser supplies continuation IDs. Verify and TimeBudget may resume
  through the existing route, model, harness, conflict, and best-of gates.
  Resume rejection falls back within the same attempt and accounts usage once.
- A resumed brief contains the current criteria, checks, verification commands,
  and plan. Removed requirements appear only in labeled history. Prompt
  instructions cover local planning and diagnosis without adding another
  advisor, judge, or replanning stage. Skills, delegation flags, permissions,
  and the native sandbox policy are unchanged.

## Canonical owners and actual consolidation

`task/` replaces the old mutable model facade. `engine/planning/` owns planning;
`harness/` owns native processes and sessions; `verify/` owns check execution and
evidence; `delivery/` owns Git delivery operations; `api/` owns protocol and MCP
adapters; `store/` owns persistence. `engine/mod.rs` declares modules rather than
providing a production import umbrella. Engine orchestration is split into
planning, attempts, lifecycle, verification, graph, questions, permissions, chat,
audit, and PR operations with explicit imports.

The old model, brief, harness, Git, MCP, RPC-task, task-loop, base-check,
verification, healing, plan, graph, permissions, questions, and PR monolith paths
are removed. Most of that code was relocated, not deleted. Remaining lifecycle
code is still oversized and is a draft blocker. IO orchestration remains in
engine where it requires the existing application lock and persistence owner.

Actual removals and consolidations include ERROR_STREAK and its normalization
state; the post-timeout report model invocation, sole prompt, and golden;
duplicate criterion/check edit paths; duplicate pending-amendment publication;
three interruption diff branches; and duplicate temporary PR checkout cleanup.
The separate fallible evolution log writer and best-effort store log writer
retain their different contracts. Character-count clipping and byte-count
clipping likewise remain distinct.

No new workspace crate, actor, revision or generation field, storage migration,
generic mutable context, or artificial request bag was introduced.

## Verification checkpoint

The complete workspace, including every test target, compiles. Targeted native
and replay proofs include:

| Behavior                                           | Actual result |
| -------------------------------------------------- | ------------- |
| Planner publication and stop/authority boundaries  | 5 passed      |
| Original owner-amendment integration cases         | 10 passed     |
| Dirty-checkout wait and rereview integration       | 2 passed      |
| Answer-healing publication and retained B receipt  | 5 passed      |
| PR completion                                      | 32 passed     |
| PR carry and owner-stop retry                      | 9 passed      |
| Public shared-check authority RPC                  | 1 passed      |
| Time-budget and continuation runner cases          | 9 passed      |
| Best-of native cases                               | 4 passed      |
| Completion receipts, restart, cancellation         | 4 passed      |
| Concurrent owner amendments and screenshots        | 4 passed      |
| Delivery and queued carry admission                | 6 passed      |
| Late amendment during real final checks            | 1 passed      |
| Dependency amendment versus first native attempt   | 1 passed      |
| Genuine task start versus accepted owner amendment | 1 passed      |

The ten-case amendment binary includes the publication and direct-store finish
fixes. It predates only the final cancellation-cleanup parity change, which
requires the separately pinned final CI to establish current-source coverage.

The task-start regression fails with the old start body: a successful Owner RPC
is overwritten by the stale task copy. It passes with the locked fresh read,
mutation, and save. The production body is the only difference in that negative
proof. Existing parent-check coverage runs the actual parent loop and commits
the newly accepted Owner check marker.

The generic AST checker has 32 synthetic tests, including negative oversized,
forbidden dependency, macro, alias, lexical shadowing, and configuration cases.
The final overall CI, full workspace suite, desktop smoke, native Claude canary,
and remote check verdicts are recorded with the final checkpoint evidence.
They are not implied by the targeted results above. The final pinned CI,
native canary, and remote checks were pending when this checkpoint was
committed; the PR body and local owner report record their later verdicts.

Independent review completed one full pass and one P0/P1 delta. The five P1
findings concerned idle amendment/start races, pending retirement, queued carry,
screenshot identity, and stopped/stale PR publication. The genuine start caller
found in the delta has a separate negative and positive regression. The
additional installed Codex review was refused by automatic approval review
because it could transmit repository source to an external service/account;
that command did not run and was not bypassed.

The full diagnostic suite subsequently exposed planner publication, relay-base
publication, and early-delivery dirty-wait regressions. Their changes have
targeted proof or an explicitly pending immutable replay. A read-only follow-up
also identified an unproven stop-versus-failure-save risk in legacy
`draft_run::fail_run` and `correction::settle_correction_timeout`: their final
failure save is not covered by the new parsed-result publication lock. No
universal stop-atomicity claim is made for those writers. The frozen integration
source is fingerprinted for the final CI and native canary.

## All twelve research recommendations remain in the plan

| #   | Recommendation                                 | This checkpoint and next step                                                                                                                                                                                                 |
| --- | ---------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | One criterion/check contract                   | Pure mutation owner and strict origin handling now; stable criterion/check IDs and a normalized catalog require a separately reviewed schema migration.                                                                       |
| 2   | Environment capability preflight               | Next: prove sockets, network, browser, tools, and writable paths in the actual sandbox before spending an implementation attempt.                                                                                             |
| 3   | One state writer                               | Pure Task operations now; an actor with revision and run generation remains a separate step.                                                                                                                                  |
| 4   | Approved harness capabilities                  | Existing skills, delegation, and permission policy preserved; evaluate approved repository skills and bounded assistants inside the same budget.                                                                              |
| 5   | Less mandatory process for small tasks         | Remove the timeout report helper now; measure existing presets before reducing planner, plan-review, or brief-check stages.                                                                                                   |
| 6   | One exception/recovery path                    | Next: typed implementation, environment, unknown, and owner outcomes with bounded recovery and explicit retry conditions.                                                                                                     |
| 7   | Separate candidate readiness and delivery      | Current gates and existing commit receipts are strengthened; explicit candidate, verified version, and persisted delivery state remain separate work.                                                                         |
| 8   | Process and resource ownership                 | Existing cancellation preserved; next add owned process/temp/socket/port scopes, evaluate Linux cgroup/systemd support, and calibrate heavy-check parallelism.                                                                |
| 9   | Small harness adapter contract                 | Native continuation remains on the existing adapter; next unify run/resume/cancel/events/usage/permissions and pilot app-server with CLI fallback.                                                                            |
| 10  | Optional test-runner evidence adapters         | Preserve code-owned gates; next move runner-specific structured evidence out of orchestration while letting the agent investigate unknown failures.                                                                           |
| 11  | Measure useful results and owner interventions | RPC/runner replay and one canary provide correctness evidence, not a first-pass benchmark. Next compare identical cases/base/environment with engine/harness/model fingerprints, owner actions, time, failures, and overhead. |
| 12  | Prompts, skills, and a few profiles            | Compact executor instructions now; extend existing profiles after measurement, without a workflow DSL.                                                                                                                        |

The follow-up order remains: environment and resource ownership (#2/#8),
contract/state/delivery (#1/#3/#7), recovery/evidence (#6/#10), matched evaluations
and process reduction (#11/#5), approved skills/profiles (#4/#12), then adapter
pilots (#9). SQLite and managed cloud remain outside this change.
