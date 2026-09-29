## Orchestrator: v1 freeze (breaking)

**Breaking:** the variant flags `retryMode`, `plannerTier`, `contract`,
`reviewOtherFamily`, `deferHeavyChecks`, `leanOutput` and `reviewBlind` are
removed. Their winning behaviour is now plain: a retry starts a fresh session
with the full brief, earlier handoffs and the last failure; the planner's tier
routes the task; the planner writes a contract and splits slow whole-repo checks
into `finalVerify`; the reviewer rules on every criterion and, with review
`auto`, runs on the other harness; the reviewer sees the implementer's
account. `leanOutput` (the rtk hook) and `reviewBlind` are deleted with their
code. An old `task.json` or `settings.json` that names one still loads and the
key is ignored; a `task.create` variant override that names one is rejected
with a message saying it was retired.

- Before the first implement attempt, orchd runs each `finalVerify` check on
  the base commit (cached per base sha and command). A check that fails is run
  once more, so a load-flaky test does not count; one that fails both times
  asks the pre-existing failure question up front, before any attempt is
  spent. `verify` commands are not checked: they are usually the tests the
  work is meant to turn green.
- The review brief carries the previous attempt's review findings, and the
  reviewer marks each finding it reports `repeat: true|false`. The first
  attempt whose review fails on a repeated finding runs the advisor and tiers
  up; the second waits with the "attempts keep failing" question.
- The plan stage no longer calls the classifier (the Jev preflight and its
  decision line are gone), and the `task.preflight` RPC and `task_preflight`
  MCP tool are removed.
- `bestOf`, `groundedChecks` and `batchQuestions` stay available and off by
  default. The evolution loop keeps recording signals; proposals start only
  when `evolution_run` is called at the owner's request.
