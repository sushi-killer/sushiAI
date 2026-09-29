## orchd: scoped checks

- `settings.scopedChecks` lists commands that only run when the task's diff
  touches their paths. Each entry is `{repo?, command, paths}`: `command` is a
  glob over the whole command string, `paths` are globs over repo-relative
  changed files. A matching command with no changed file under its paths is
  skipped everywhere it would run (attempt verify and finalVerify, the Stop
  hook, landing, a parent's own checks, best-of) and the task gets one line,
  `Orchestrator: skipped <command>: no change under <paths>`. The base
  preflight has no diff, so it uses the task's planned `paths`. The default
  entry runs `npm run test:desktop` only for `src/app/**`, `src/extensions/**`,
  `electron/**`, `src/styles/**` and `*.html`. The plan brief lists the
  applicable entries and tells the planner not to add such a command to
  `verify`/`finalVerify` when the task's paths fall outside its globs.
