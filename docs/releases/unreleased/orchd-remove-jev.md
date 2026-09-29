## Orchestrator: Jev and the classifier settings are removed

- orchd no longer calls an external classifier (Jev). The `classifier` setting,
  its secrets, the `classify.probe` call and the `decisions.jsonl` log are
  gone; an existing `decisions.jsonl` is left on disk untouched.
- A task with no planner tier runs on the `standard` route and records
  `no planner tier` as its tier fallback.
- A blocked agent report now goes straight to the owner, through the
  orchestrator's triage when that is on, instead of first asking a classifier
  whether the agent could answer it alone.
- The Stop hook checks only for changed files and failing verify commands; it
  no longer asks a classifier whether unverified work is finished.
- An older `settings.json` or `secrets.set` payload that still carries
  classifier fields loads as before; the fields are ignored.
