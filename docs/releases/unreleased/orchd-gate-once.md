## Orchestrator

- The planner now drafts every part of a split task completely (title, goal, criteria, verify, finalVerify, paths, tier). A part with a title, goal, criteria and verify runs as drafted and skips its own planner; a part with an incomplete draft is still planned from its request.
- A dependent part drafted by the parent is refreshed once after the tasks it builds on landed, when they changed a file under its paths or one its check commands name: one cheap run on the brief-check route may correct its verify, finalVerify and paths. It never changes the goal or criteria, and a rewrite that removes a shared check is refused.
- A task asks one base-check question listing every check that fails on the base, with drop-all and keep-all options. A failing check whose targets the plan creates is kept by rule, with no question.
- A whole-repo check a subtask shares with its parent runs once at the parent, on the final tree (`gateAt`, `"parent"` by default); a command that already passed on exactly that tree is not run again.
- Review `auto` never picks the route that did the work and prefers a route on another agent CLI.
- A parent's `finalVerify` base runs go to the background, so the first subtask starts at once.
