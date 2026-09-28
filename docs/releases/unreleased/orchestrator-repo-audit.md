## Orchestrator: agent-readiness audit for any repository

- orchd can audit a repository for autonomous agent work: `repo.audit {repo, route?}` (and the orchestrator agent's `repo_audit` tool) starts one read-only run, on the planner's route by default, that grades the repository against a fixed rubric - instructions, build and test commands, test health, legibility, context hygiene, safety, and verification surfaces for a reviewer.
- The run has a review's restrictions: it only reads the repository's own checkout, edits nothing and gets no extra network. It returns an audit id at once and streams progress like any other run.
- The report lists a grade (good, weak, missing), `path:line` evidence, a concrete recommendation and an effort estimate per area, plus up to five top fixes. Checks the agent could not run safely are marked unmeasured, and a reply without a parsable report is stored as an error, never filled in.
- `repo.audit.get {id}` and `repo.audit.list {repo}` return audits with their reports and cost. There is no panel for them yet.
- The audit runs Claude without the audited repository's own
  `.claude/settings.json` / `settings.local.json`: their hooks are shell
  commands that would run outside the sandbox in that checkout. The repo must
  be a git repository; it is recorded at its top level.
