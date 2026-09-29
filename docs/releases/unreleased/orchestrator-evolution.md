# Orchestrator: evolution proposals

The orchestrator page now lists orchd's evolution proposals for the current
repo under PROPOSALS, and the renderer can reach `evolution.run`, `.list`,
`.approve`, `.reject` and `.adopt`.

## The loop

detect -> cluster -> propose -> gate -> adopt -> measure

1. **Detect**: when a task ends done or failed, orchd records the signals it
   left behind (loops, throwaway scripts, repeated review findings, owner
   questions, ...).
2. **Cluster**: signals with the same cause are grouped; a cluster needs
   enough wasted calls or spend to be worth a proposal.
3. **Propose**: a read-only proposer run writes one proposal per cluster:
   track, form, change, evidence, metric.
4. **Gate**: the proposal is checked before it is stored.
5. **Adopt**: see the two tracks below.
6. **Measure**: later tasks are compared with the ones before the adoption.

## Two tracks

- **Repo track**: a change to the audited repository. Approve creates and
  starts an ordinary task; the proposal is adopted when that task is done.
- **Harness track**: a change to orchd's own settings or prompts. It carries an
  `orchd eval run` A/B command and is adopted only through `evolution.adopt`,
  from the panel's Mark adopted button or the CLI.

## Gates

- Nothing is applied without approval.
- Eval tasks produce no signals.
- A proposal that weakens a check, gate or protected path, or that targets
  AGENTS.md/CLAUDE.md prose or memory, is stored as rejected with a reason.
- `revert_suggested` flags an adopted change whose metric regressed; the panel
  shows it in the danger tone.
- `orchd evolve` runs a round from the command line (`--adopt <id>` marks a
  proposal adopted).
