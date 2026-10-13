## Orchestrator

- Planner tests stay clean for the repo's own format command. After a carry onto a new base, orchd sets the agent's work aside, runs the `format` command from `.sushiai/setup.json` over the planner tests on the new base, folds the result into the planner commit, refreshes the protected hashes and brings the work back. A rewritten base or a newer formatter no longer leaves a protected test that fails the format check.
