## Orchestrator

- A task's detail now shows its pipeline as a diagram: Brief, Plan, Brief check, Base checks, Implement, Checks, Held-out check, Review, Judge and Land, with Advisor and the open question as a branch. The running stage is highlighted, a failed stage is marked, and each stage shows its model, time and cost.
- New Pipeline view in the Orchestrator rail: the repo's pipeline as the same diagram, with a preset picker and an inspector. A click on a stage opens its switch, route and maximum attempts. A task's own diagram is read-only.
- Three built-in presets: Fast (no brief check, advisor or held-out check, 2 attempts), Careful (all three on, 4 attempts) and Docs (brief check only, 2 attempts). Editing a built-in preset makes an editable copy.
- A preset can be set per repo (`repoPresets`) and per task (`preset` on `task.create`, or `task.preset` before the first implement attempt). Shared checks stay owner-only under every preset.
