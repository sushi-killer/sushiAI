# orchd task-acceptance audit

What decides that an orchd task is `done`, which model or route makes each of
those decisions, and every confirmed way weak or unverified work can still get
committed. Code citations are against `61e52b0` (the base of
`chore/orchd-acceptance-audit`). This is a docs-only audit: nothing under
`orchd/src/`, `src/` or `electron/` was changed.

"Accepted" here means what `run_task_loop` does at the end
(`orchd/src/engine.rs:3729-3759`): `git add -A && git commit` on the task
branch, attempt `passed`, task `done`. Nothing after that step re-checks the
work. Merging to `main` is still the owner's call.

**Status (2026-09-24, `d26de58`):** G1 closed (no verdict asks the owner),
G2 closed (a skipped review is recorded as a decision), G3 closed (auto
review runs on the hard tier and sees verify output and the implementer's
report); G11 is now visible to the reviewer instead of gated. G4-G10 are
still open.

## Pipeline order

For one task, `run_task_loop` (`engine.rs:3032`) runs:

1. **Plan/draft**, only for the `{repo, request}` form, and at most once
   (`needs_planning`, `engine.rs:2608-2614`; called at `engine.rs:3077`).
2. **Tier classification**, once, before the first implement attempt
   (`engine.rs:3104-3108`).
3. **Implementation** on `tiers[task.tier]` (`engine.rs:3112-3132`).
   - The **in-session Stop hook** runs while the Claude session is still
     live (`handle_hook_stop`, `engine.rs:1068`).
4. **Post-run verify gate** (`engine.rs:3556-3602`).
5. **Protected-path gate** (`engine.rs:3604-3658`).
6. **Review** (`engine.rs:3660-3727`).
7. **Commit**, which marks the task `done` (`engine.rs:3729-3759`).

**Stuck-question triage** runs whenever steps 1 or 3 park on a question. The
**orchestrator chat** sits outside the loop and drives it through `orchd mcp`.

## Stage-by-model matrix

| Stage                                           | Code                                                                                        | Route / model setting                                                                                                                      | Default                                                                                                                                                                                                                                               | Access                                                                                                                                                     | If this stage fails or is unavailable                                                                                                                                                                                                                                                                |
| ----------------------------------------------- | ------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Plan/draft                                      | `run_plan_stage` `engine.rs:2647`; brief `brief::build_plan_brief` `brief.rs:168-182`       | `Settings.planner`, a route id (`model.rs:102-106`)                                                                                        | `"claude-sonnet"` (`default_planner`, `model.rs:119-121`), which is Claude, `--model sonnet`, no effort                                                                                                                                               | Read-only (`review: true`, so `Read,Grep,Glob` and plan mode, `engine.rs:2752-2762`, `harness.rs:51-55`)                                                   | Unknown or empty id: create is rejected up front (`engine.rs:729-750`) or the task fails (`engine.rs:2681-2697`). No sushi-plan block after 1 retry: owner asked, up to 2 rounds (`engine.rs:2839-2887`). Harness error: task `failed` (`engine.rs:2801-2828`).                                      |
| Plan preflight (part of plan)                   | `engine.rs:2900-2948`                                                                       | `Settings.classifier`                                                                                                                      | backend `openrouter`, model `typesafe/jev-1.13` (`model.rs:158-162`), **no key** by default                                                                                                                                                           | Network call, secrets redacted                                                                                                                             | Advisory only: it writes one `Jev: goal …, criteria …, verification …` decision line and never blocks anything                                                                                                                                                                                       |
| Tier classification                             | `classify_tier` `engine.rs:2257-2303`                                                       | `Settings.classifier` + classifier key from `secrets.set`                                                                                  | same as above: openrouter / `typesafe/jev-1.13`, no key, 5 s timeout (`classify.rs:13`)                                                                                                                                                               | Network call on `{goal, criteria}` only (`engine.rs:2261`)                                                                                                 | **Falls back to `standard`** on any error, timeout, missing key, missing choice or `p < 0.5` (`engine.rs:2279-2295`). See G8.                                                                                                                                                                        |
| Implementation                                  | `run_task_loop` `engine.rs:3032`, route pick `engine.rs:3110-3132`                          | `Settings.tiers[task.tier]`, which goes up one tier after the same failure signature repeats (`decide_after_failure`, `engine.rs:124-139`) | mechanical → `codex` (Codex CLI default model), standard → `claude-sonnet`, hard → `claude-opus` with effort `high` (`model.rs:125-156`). Unknown tier or route id: silently `codex` (`engine.rs:3116`, `3125-3132`)                                  | Writes. Claude uses `acceptEdits` (`harness.rs:56-59`); Codex uses `workspace-write` (`harness.rs:128-135`)                                                | Harness error with no files changed: failure `error`. With files changed, the error is ignored (G11).                                                                                                                                                                                                |
| Verify gates: in-session Stop hook              | `handle_hook_stop` `engine.rs:1068-1173`, decision `hook::decide_stop` `hook.rs:53-90`      | No model when `task.verify` is non-empty. Otherwise `Settings.classifier` with the "jev-belay" questions (`engine.rs:1199-1224`)           | **Claude implement runs only.** The hook is registered only for `Harness::Claude` (`engine.rs:3229-3287`), so a Codex attempt has no in-session gate at all                                                                                           | Runs `task.verify` in the verify sandbox                                                                                                                   | Allows when: no files changed; 3 blocks already used (`hook.rs:54`); verify times out after 540 s (`engine.rs:1116`); verify is empty and the classifier is unavailable or unparseable (G10)                                                                                                         |
| Verify gates: post-run gate                     | `engine.rs:3556-3602` via `run_verify_cached` `engine.rs:1398-1427`                         | No model. Runs `task.verify` exactly as stored                                                                                             | Whatever the caller or planner put in `task.verify`, which **may be empty** for the direct `task.create` form (`engine.rs:790-791`)                                                                                                                   | Sandboxed shell (`build_verify_command`)                                                                                                                   | Any non-zero exit: failure `verify`. Empty list: nothing runs, nothing fails (G4)                                                                                                                                                                                                                    |
| Protected-path gate                             | `engine.rs:3604-3658`, `matches_any_protected` `engine.rs:181-183`                          | No model. Owner answer only                                                                                                                | `protected_paths: []` (`model.rs:173`), so the gate never fires                                                                                                                                                                                       | n/a                                                                                                                                                        | Anything but an exact `"approve"` rejects (`engine.rs:3628`)                                                                                                                                                                                                                                         |
| Review                                          | `run_review` `engine.rs:2337-2440`; route pick `select_review_route` `engine.rs:188-197`    | `Settings.review`: `""` = off, `"auto"`, or a route id (`model.rs:88-90`)                                                                  | `"auto"` (`model.rs:157`): the **first route whose harness differs** from the implementer's. With the default routes that is `codex` (no model, no effort) for any Claude implement run, **including hard/Opus**, and `claude-sonnet` for a Codex run | Read-only (`review: true`); diff capped at 60 000 chars (`engine.rs:2352`); only verify exit codes are shown, never output (`engine.rs:2365-2368`)         | **Harness error, unparseable output, or failure to spawn are all treated as `PASS`** (`engine.rs:2421-2438`) (G1). No matching route means review is silently skipped (`engine.rs:3662`) (G2)                                                                                                        |
| Stuck-question triage: classifier               | `classify_answerable` `engine.rs:2305-2330` → `decide_blocked_question` `engine.rs:148-153` | `Settings.classifier`                                                                                                                      | as above (no key)                                                                                                                                                                                                                                     | Network call                                                                                                                                               | Runs **regardless of `auto_answer`**. `p ≥ 0.7`: the agent is sent back to "answer it yourself" with no owner involved (`engine.rs:3457-3478`). Error: goes to the owner                                                                                                                             |
| Stuck-question triage: orchestrator auto-answer | `run_triage` `engine.rs:1960-2056`, wrappers `engine.rs:2077-2209`                          | `Settings.orchestrator` → `chat::orchestrator_route` (`chat.rs:175-178`): that id, else `tiers.standard`                                   | `auto_answer: false` (`model.rs:112-116`), so off. When on, the route is `claude-sonnet`                                                                                                                                                              | Read-only, no MCP servers (`engine.rs:1989-2016`)                                                                                                          | Triage only answers the agent's own blocked question and the planner's draft questions. Never the protected-path or attempts-exhausted questions. At most one answer in a row (`engine.rs:1975-1982`). `sanitize_triage` refuses `approve`/`stop` (`brief.rs:357-405`). Errors escalate to the owner |
| Orchestrator chat                               | `chat::handle_send` `chat.rs:130-171`, argv `chat.rs:197-305`                               | Same `chat::orchestrator_route`                                                                                                            | `claude-sonnet` (standard tier), because `orchestrator: ""` (`model.rs:107-111`)                                                                                                                                                                      | `Read,Grep,Glob` plus 8 MCP tools, **including `task_answer`, `task_create`, `task_start`** (`chat.rs:19-28`, `203-230`); Codex runs `--sandbox read-only` | Its limits ("never answer approve", "never call task_stop") are **prompt text only** (`mcp.rs:39-43`) (G7)                                                                                                                                                                                           |

Tiers are not the same as the models that judge the work. With default settings
and no classifier key, **every** task is implemented by Sonnet (G8). Its plan
is drafted by Sonnet. Its review runs on the Codex CLI's default model. No
stage's strength follows task difficulty except implementation, and
implementation only after a classifier call succeeds.

## Reproduction setup

Every reproduction below uses the same approach as `orchd/tests/integration.rs`:
the real daemon, fake harness binaries set through `ORCHD_CLAUDE_BIN` /
`ORCHD_CODEX_BIN` (`resolve_binary`, `engine.rs:1558-1565`), and NDJSON over
the unix socket. You can paste each one as a new `#[test]` using the existing
`Daemon::spawn`, `fake_harness_script`, `init_git_repo` and `poll_task_status`
helpers. Or, by hand:

````sh
cargo build --manifest-path orchd/Cargo.toml
D=$(mktemp -d); S=$D/orchd.sock; F=$(mktemp -d)

# Fake Claude implementer: edits one file, reports "complete".
# (Same body as the engine_loop_passes_when_verify_succeeds fake,
# orchd/tests/integration.rs:317.)
cat > $F/claude.sh <<'EOF'
#!/bin/sh
cat > /dev/null
echo changed > CHANGED_MARKER.txt
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
EOF
chmod +x $F/claude.sh

ORCHD_CLAUDE_BIN=$F/claude.sh ORCHD_CODEX_BIN=${CODEX_FAKE:-/nonexistent/codex} \
  orchd/target/debug/orchd serve --data $D --socket $S &
TOK=$(cat $D/control.token)
rpc() { node -e 'const s=require("net").connect(process.argv[1]);
  s.write(JSON.stringify({id:"1",method:process.argv[2],params:JSON.parse(process.argv[3]),auth:process.argv[4]})+"\n");
  s.on("data",d=>{process.stdout.write(d);s.end()})' "$S" "$1" "$2" "$TOK"; }

R=$(mktemp -d); git -C $R init -q; echo x > $R/README; git -C $R add .; git -C $R commit -qm init
````

"Set X" means: `rpc settings.get '{}'`, change the field in the returned
object, then `rpc settings.set '{"settings": <object>}'`. Tasks are read back
with `rpc task.get '{"id":"<id>"}'`. Classifier calls are journaled to
`$D/decisions.jsonl` (`engine.rs:486-507`).

## Gaps

Each gap is confirmed from the code path cited, with a reproduction. **Owner**
says whether `fix/orchd-stronger-review` is supposed to cover it. The
[overlap check](#overlap-with-fixorchd-stronger-review) explains how that was
determined.

### G1. Review error, unparseable output or IO failure counts as PASS

- **Code:** `run_review`, `engine.rs:2421-2438`. `Ok(outcome)` with no
  parseable `sushi-review` block becomes `Verdict::Pass` with the finding
  `"review did not run (…); treated as pass"` or
  `"review output unparseable; treated as pass"`. `Err(RunError::Io)` (the
  binary failed to spawn) becomes `Verdict::Pass` with `"review session failed
to run (…); treated as pass"`. The caller only fails on
  `Verdict::Fail` (`engine.rs:3699-3700`). Its `Err(RunError::Io(_)) =>
review_result = None` arm (`engine.rs:3691-3692`) can never be reached,
  because `run_review` never returns `Io`.
- **Reproduction:** leave `review: "auto"` (the default), with the fake Claude
  above and `ORCHD_CODEX_BIN=/nonexistent/codex`.
  `rpc task.create '{"repo":"'$R'","title":"t","goal":"g","verify":["true"],"start":true}'`.
  The task ends `done` with a commit on its branch.
  `attempts[0].review = {"verdict":"PASS","findings":["review session failed to run (/nonexistent/codex: No such file or directory …); treated as pass"]}`.
  Variants: set `CODEX_FAKE` to a script that is just `cat >/dev/null; exit 0`,
  which gives "review output unparseable". Make it `exit 1`, which gives
  "review did not run (Codex exited with …)". Either way the task is `done`.
- **Owner:** `fix/orchd-stronger-review` (review-failure handling).

### G2. Review silently skipped when no route qualifies

- **Code:** `engine.rs:3661-3662`. `if let Some(review_route) =
select_review_route(..)` has no `else`. `select_review_route`
  (`engine.rs:188-197`) returns `None` in two cases: `"auto"` when every
  configured route uses the implementer's harness, and an explicit id that
  matches no route. The attempt is committed with `review` absent. The owner
  sees no line saying review was skipped.
- **Reproduction:** set `routes` to just the two Claude routes (drop `codex`)
  and `tiers.mechanical` to `"claude-sonnet"`, keeping `review: "auto"`. Or
  keep all routes and set `review: "claude-hiaku"` (a typo). Create the task
  from G1. It ends `done`, `attempts[0]` has no `review` key, and there is no
  decision line.
- **Owner:** `fix/orchd-stronger-review` (review-failure handling / review
  strength).

### G3. Review is weaker than the work it judges, and sees less

- **Code:** `"auto"` picks the first route with a different harness
  (`engine.rs:188-197`). With the default routes (`model.rs:130-155`), a
  `hard` task implemented on `claude-opus` with effort `high` is reviewed by
  `codex` with `model: None` and `effort: None`. The review brief carries the
  diff truncated to 60 000 chars (`engine.rs:2352`) and only
  `command -> exit code` for verify (`engine.rs:2365-2368`), so the reviewer
  cannot see what the verify commands actually printed.
- **Reproduction:** set `CODEX_FAKE` to
  `#!/bin/sh\ncat >/dev/null\nprintf '%s\n' "$@" > /tmp/review-argv`. Force
  the hard route by setting `tiers.standard` to `"claude-opus"`, then create
  the task from G1. `/tmp/review-argv` shows `exec --json … --sandbox
read-only -` with no `--model` and no `model_reasoning_effort`. The review
  "passes" per G1.
- **Owner:** `fix/orchd-stronger-review` (review strength).

### G4. No verify gate is required, so empty `verify` passes the post-run gate

- **Code:** `handle_task_create` accepts `verify: []` for the direct form
  (`engine.rs:713-714`, `790-791`). The post-run gate iterates an empty list
  and finds no failure (`engine.rs:3556-3567`, `run_verify_commands`
  `engine.rs:1386-1392`). The plan stage asks for a command when the draft
  has none (`engine.rs:2980-3003`), but only for the `{repo, request}` form,
  and it accepts any free-text answer.
- **Reproduction:** set `review: ""`. Run
  `rpc task.create '{"repo":"'$R'","title":"t","goal":"g","criteria":["x works"],"verify":[],"start":true}'`.
  It ends `done`, `attempts[0].verify = []`, and it is committed. Nothing
  checked "x works".
- **Owner:** `fix/orchd-stronger-review` (required verify gate).

### G5. The planner is told to pick the fastest command, and nothing checks that the command covers the criteria

- **Code:** `PLAN_INSTRUCTIONS` (`brief.rs:168`): "Verify commands must be the
  fastest ones that already exist … prefer a targeted test over a full CI
  run." Nothing asks for `npm run ci` or for a command per criterion. The
  plan preflight does ask exactly that question ("Do the verification
  commands actually exercise the criteria?", `engine.rs:2921-2924`), but its
  answer is only logged as a `Jev:` line (`engine.rs:2939-2947`) and gates
  nothing. The follow-up verify question fires only when `verify` is empty
  (`engine.rs:2980`), never when it is merely weak. The implementer brief
  then says a denied or unavailable verify command "is not a reason to stop"
  (`RULES_BLOCK`, `brief.rs:35`).
- **Reproduction (live):** this audit task itself. Its planner drafted
  `verify: ["npm run check:conventions", "git status -sb"]`. The preflight
  scored it `Jev: goal 0.76, criteria 0.68, verification 0.20`, and the task
  was queued and implemented anyway. `git status -sb` exits 0
  unconditionally. `scripts/check-conventions.mjs` never reads this file (it
  only looks at `docs/LESSONS.md` and release fragments). So an empty
  `docs/orchd-acceptance-audit.md`, or one missing the required matrix, would
  pass both gates. Only review stands between it and `done`, and review is
  subject to G1–G3.
- **Reproduction (synthetic):** use a fake planner. `FAKE_PLANNER_SCRIPT`,
  `orchd/tests/integration.rs:656`, returns a draft with
  `"verify":["true"]` for any request. With a classifier key set, the
  preflight line records a low `verification` score. No question is asked,
  and the task proceeds to implement and commit.
- **Owner:** open. A required verify gate (G4) forces a non-empty list. It
  does not force a meaningful one.

### G6. `protected_paths` defaults to empty, and verify runs the agent's own edited copy

- **Code:** `Settings::default` sets `protected_paths: vec![]`
  (`model.rs:173`). The UI shows it as an empty textarea
  (`src/orchestrator/OrchestratorSettings.tsx:170`). With no globs,
  `matches_any_protected` is always false (`engine.rs:181-183`, `3604-3606`),
  so no change ever reaches the owner before commit. Verify commands run in
  the agent's worktree (`run_verify_commands(cwd = worktree, …)`,
  `engine.rs:1421`). That means the agent can edit the definition of its own
  gate (`package.json` scripts, test files, `scripts/check-conventions.mjs`,
  `eslint.config.mjs`) and the gate runs the edited version. Changes to
  `src/app/*`, `electron/main.cjs` or the manifest validator, which AGENTS.md
  says need the owner's approval, also go through unasked.
- **Reproduction:** commit a `check.sh` containing `exit 1` in `$R`. Point the
  fake Claude at `echo 'exit 0' > check.sh` instead of writing
  `CHANGED_MARKER.txt`. Set `review: ""` and leave `protectedPaths` at its
  default. Run
  `rpc task.create '{"repo":"'$R'","title":"t","goal":"make check pass","verify":["sh check.sh"],"start":true}'`.
  It ends `done`, `attempts[0].verify[0].code = 0`, and the commit rewrites
  the check. Rerun with `protectedPaths: ["check.sh"]`: the task parks
  `waiting` on "Change touches protected path check.sh: approve or reject?".
  The integration test `protected_path_approval_is_never_triaged`
  (`integration.rs:1547-1560`) only passes because it sets the list
  explicitly.
- **Owner:** open.

### G7. The orchestrator chat can answer owner-only questions, and the answer is recorded as the owner's

- **Code:** the chat agent gets `mcp__sushiai-orchestrator__task_answer`
  (`chat.rs:19-28`, `203-207`). `orchd mcp` authenticates with the daemon's
  single control token (`read_control_token`, `mcp.rs:304`). `check_auth`
  cannot tell the chat apart from the app (`engine.rs:458-463`).
  `handle_task_answer` (`engine.rs:889-948`) delivers any answer to any
  waiting question. That includes the protected-path approval
  (`engine.rs:3610-3628`) and the "Attempts keep failing … continue?"
  question, where any answer adds 2 attempts (`engine.rs:1808-1812`, `1861`).
  The decision is logged as `"Owner: approve"` (`engine.rs:1899`). The only
  guard is prompt text (`mcp.rs:42-43`). The code-level `approve`/`stop`
  refusal (`sanitize_triage`, `brief.rs:381`) applies only to `run_triage`,
  not to this path.
- **Reproduction:** set `review: ""` and `protectedPaths: ["CHANGED_MARKER.txt"]`,
  then create the task from G1. When it is `waiting` on the protected-path
  question, send exactly what the chat agent's tool call sends:
  ```sh
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"task_answer","arguments":{"id":"<id>","answer":"approve"}}}' \
    | orchd/target/debug/orchd mcp --data $D --socket $S
  ```
  The task goes `done` and `decisions` ends with `"Owner: approve"`. Nothing
  records that the chat agent, not the owner, approved it.
- **Owner:** open.

### G8. Tier classification silently falls back to `standard`

- **Code:** `classify_tier` (`engine.rs:2257-2303`) returns `Tier::Standard`
  in every one of these cases: `decide` returns `Err`, which covers a missing
  key (`classify.rs:337`), backend `none` (`classify.rs:342`), network error,
  non-2xx or the 5 s timeout (`classify.rs:13`, `276-291`), and a missing
  `answers` object (`classify.rs:241`); no `tier` answer; no `choice`; or
  `p < 0.5`. On an `Err` no decision line is written (`engine.rs:3117-3119`
  only runs for `Some`). The error lands only in `decisions.jsonl`, and not
  even there when the backend is `none` (`engine.rs:494-496`). A missing
  probability counts as full confidence (`unwrap_or(1.0)`, `engine.rs:2293`).
  A low-confidence `hard` writes the misleading line `Jev: tier hard (p 0.45)
-> route claude-sonnet`. Out of the box the classifier is `openrouter` with
  no key (`model.rs:158-162`), so **every task is `standard`**. Tier only
  rises after two consecutive failures with the same signature
  (`engine.rs:133-137`), so a hard task that first produces weak but
  _passing_ work on Sonnet is never escalated.
- **Reproduction:** start from default settings with no `secrets.set` and
  `review: ""`. Run
  `rpc task.create '{"repo":"'$R'","title":"Rewrite the scheduler","goal":"Replace the engine concurrency model with a lock-free design","verify":["true"],"start":true}'`.
  The result has `tier: "standard"`, `attempts[0].routeId: "claude-sonnet"`,
  `attempts[0].reason: "tier standard -> route claude-sonnet"`, and no
  `Jev: tier` decision. `grep '"point":"tier"' $D/decisions.jsonl` shows
  `"error":"no classifier key configured"`. For the low-confidence variant,
  use the fake OpenAI classifier from `integration.rs:1251` with
  `{"answers":{"tier":{"choice":"hard","probabilities":{"hard":0.45}}}}`,
  settings `classifier: {"backend":"openai",…}`, and
  `secrets.set {"classifier":{"key":"k","baseUrl":"<url>"}}` (same setup as
  `integration.rs:1318-1329`). You get `tier: "standard"` and the decision
  `Jev: tier hard (p 0.45) -> route claude-sonnet`.
- **Owner:** open.

### G9. The planner runs on a fixed route, before and independent of tier classification

- **Code:** `run_plan_stage` runs at `engine.rs:3077-3098`. `classify_tier`
  runs afterwards at `engine.rs:3104`, and its only input is the planner's own
  `goal`/`criteria` (`engine.rs:2261`). The planner route is always
  `settings.planner` (`engine.rs:2681-2685`), `claude-sonnet` by default
  (`model.rs:119-121`). Planning happens once per task (`needs_planning`,
  `engine.rs:2608-2614`). So for a task later classified `hard`, the
  acceptance criteria and verify commands that gate the Opus implementer were
  written by Sonnet, and nothing ever revisits them. That includes the
  "fastest command" choice from G5. It also means a weakly drafted goal
  makes a weak `hard` signal more likely, which feeds into G8.
- **Reproduction (live):** this task's `decisions` read `Jev: goal 0.76,
criteria 0.68, verification 0.20` (plan preflight), then `Jev: tier hard
(p 0.97) -> route claude-opus`. The plan attempt ran on
  `claude-sonnet` with `reason: "drafting"`. The implement attempt is on
  `claude-opus`.
- **Reproduction (synthetic):** use `FAKE_PLANNER_SCRIPT` as
  `ORCHD_CLAUDE_BIN` plus the fake classifier returning `hard` at `0.9`. Run
  `rpc task.create '{"repo":"'$R'","request":"<anything>","start":true}'`.
  Then `attempts[0]` is `{stage:"plan", routeId:"claude-sonnet"}` and
  `attempts[1]` is `{stage:"implement", routeId:"claude-opus"}`. Setting
  `tiers.hard` to anything has no effect on `attempts[0]`.
- **Owner:** split. The _default value_ of `planner` is claimed by
  `fix/orchd-stronger-review` (see the overlap check). The _ordering and
  independence_, where planning never follows the tier, is open.

### G10. The Stop hook allows unconditionally when `verify` is empty and the classifier is unavailable

- **Code:** in `handle_hook_stop`, the classifier path only runs when
  `verify` is empty (`engine.rs:1122-1152`). Any classifier error, or any
  missing answer field (`to_jev_belay`, `engine.rs:1226-1235`), gives
  `classifier = None`. `decide_stop` then falls through to `Allow`
  (`hook.rs:77-89`), and the unit test
  `no_verify_commands_and_no_classifier_allows` (`hook.rs:206-209`) pins that
  behavior. The hook is also skipped entirely for Codex attempts
  (`engine.rs:3229`), and it fails open when verify runs past 540 s
  (`engine.rs:1116`). With empty verify the post-run gate checks nothing
  either (G4), so the only check left is review, which is subject to G1–G3.
- **Reproduction:** set `ORCHD_CLAUDE_BIN` to the `FAKE_CLAUDE_HOOK_SCRIPT` from
  `integration.rs:457-497`, a node script that edits a file and calls the
  real Stop hook from its generated settings. Use default settings with no
  classifier key and `review: ""`. Create the direct-form task with
  `"verify":[]` and `"start":true`. The hook returns `{}`,
  `attempts[0].gateBlocks = 0`, there is no `Jev: premature finish` line, and
  the task is `done`. `$D/decisions.jsonl` has `"point":"stop_gate"` with
  `"error":"no classifier key configured"`. Compare
  `stop_hook_blocks_on_failing_verify_then_task_passes_once_fixed`
  (`integration.rs:500`): with a real verify command the same script is
  blocked once.
- **Owner:** open. It narrows to the Codex-only and timeout cases if G4's
  required verify gate lands.

### G11. A `partial` report, a missing report, or a crashed harness still gets committed

- **Code:** the only report outcome the loop acts on is `blocked`
  (`engine.rs:3418-3423`). `partial`, a missing `sushi-report`, and an
  unparseable report (`brief::parse_report` → `None`, `brief.rs:157-160`) all
  go on to verify, review and commit. `outcome.error` (a non-zero harness
  exit, a Claude `is_error` result, or a Codex `turn.failed`) is only
  consulted when _no_ files changed (`engine.rs:3512-3515`). An attempt that
  hit a budget or API error halfway through, but had already edited files, is
  treated exactly like a finished one.
- **Reproduction:** change the fake Claude's report to `"outcome":"partial"`,
  and add `exit 1` as the script's last line. Set `review: ""` and create the
  task from G1 with `"verify":["true"]`. It ends `done` and is committed,
  with `attempts[0].summary = "done"`. Nothing records that the agent itself
  said the work was partial, or that the harness exited 1.
- **Owner:** open.

## Overlap with `fix/orchd-stronger-review`

How this was checked (2026-09-24):

- In the requested sibling checkout, the main checkout,
  `git rev-parse fix/orchd-stronger-review` returns
  `61e52b09c86f3262118a1b30bfe3e8dbe6e7e077`, the same commit as this branch's
  base. `git diff 61e52b0..fix/orchd-stronger-review -- orchd/src orchd/tests`
  is empty, and that checkout is clean (its worktree happens to be on
  `feature/inbox-tray-worktrees`). The target branch therefore has **no
  committed diff of its own** to verify. There was no uncommitted work on the
  target branch to inspect.

That leaves no diff to check the claimed scope against. The labels below
therefore come from the scope stated in this task's brief (review strength,
review-failure handling, a required verify gate, the planner default). They
are marked **claimed, not yet in the branch**. Before relying on them, rerun
`git log --oneline main..fix/orchd-stronger-review` and
`git diff main...fix/orchd-stronger-review --stat`. If the diff turns out not
to cover a claimed item, move it to the open list below.
`fix/orchd-strong-planner` also exists, with no commits either, and by its
name it may be the real owner of the planner default.

| Gap                                     | Claimed by `fix/orchd-stronger-review`   | Verified in its diff      |
| --------------------------------------- | ---------------------------------------- | ------------------------- |
| G1 review failure → PASS                | yes (review-failure handling)            | no, branch has no commits |
| G2 review silently skipped              | yes (review-failure handling / strength) | no, branch has no commits |
| G3 weak / blind review                  | yes (review strength)                    | no, branch has no commits |
| G4 no required verify gate              | yes (required verify gate)               | no, branch has no commits |
| G9 planner _default_                    | yes (planner default)                    | no, branch has no commits |
| G5, G6, G7, G8, G9 (ordering), G10, G11 | no                                       | n/a                       |

## Recommendations (open gaps only, ranked)

Ranked by how directly each gap lets unreviewed or unverified work reach
`done`, and by how many tasks it affects under default settings. Findings
claimed by `fix/orchd-stronger-review` (G1–G4 and the planner default) are
not re-recommended here.

1. **G6: ship a non-empty `protected_paths` default and protect the gate's own
   definition.** It hits every task under default settings and gives no
   signal. The agent can rewrite its own verify target. Suggested defaults for
   this repo: `package.json`, `scripts/**`, `eslint.config.mjs`,
   `src/app/**`, `electron/main.cjs`, `electron/extensions/manifest.cjs`,
   `orchd/src/**`, `AGENTS.md`. An alternative, or an addition: run verify
   against the base commit's copy of the files the commands depend on. The
   default belongs in `Settings::default` (`model.rs:173`), and a test
   asserting it must live where `npm run ci` runs it (`cargo test` via
   `test:orchd`).
2. **G8: make a tier fallback visible, and fail safe upward.** Write a
   `Jev: tier unavailable (<reason>) -> route <id>` decision on every fallback.
   Treat a missing probability as unknown rather than `1.0`. Consider falling
   back to `hard`, or to a configurable `fallbackTier`, instead of `standard`
   when the classifier is configured but failed: an unneeded Opus run costs
   money, while an under-powered Sonnet run gets committed. Fix the
   `Jev: tier hard (p 0.45) -> route claude-sonnet` wording so it names the
   tier that was actually used.
3. **G7: enforce owner-only answers in code, not prompt text.** Give the MCP
   bridge its own token or a caller tag. In `handle_task_answer`, reject
   `approve` on a protected-path question and any answer on an
   attempts-exhausted question when the call comes from that token. Record
   the answer as `Orchestrator:`, not `Owner:`.
4. **G10: stop allowing silently when neither verify nor the classifier is
   available.** Block once with "no verification command and no classifier;
   state what you ran". Or, if G4's required gate lands, remove the case
   entirely. Either way, record a decision line when the hook allows with no
   evidence. Codex attempts need an equivalent. Today they get only the
   post-run gate.
5. **G5 (+ G9): make the plan's verify choice answerable to its criteria.**
   When the preflight's `has_verification` score is below a threshold (this
   task scored 0.20), ask the owner the verify question instead of only
   logging the score. Drop "prefer a targeted test over a full CI run" from
   `PLAN_INSTRUCTIONS`, or pair it with the repo's full CI command as a
   required final gate. Plan after classifying: classify the raw request
   first, then draft on `tiers[tier]`, or on a `planner` value of `"tier"`,
   so a hard task is also planned by the hard route. Re-classify after
   drafting only if the draft changes the picture.
6. **G11: respect the agent's own report and the harness exit.** Treat
   `outcome: partial`, a missing or unparseable report, and `outcome.error`
   with changes as a failed attempt (`FailureKind::NoDeliverable` or
   `Error`), not as a candidate for commit.

## Not a gap (checked)

- **The Stop hook's 3-block cap and verify timeout** (`hook.rs:54`,
  `engine.rs:1116`) let a session _end_. They do not let work _pass_: the
  post-run gate re-runs `task.verify` (`engine.rs:3556`). The verify cache is
  keyed on the full diff plus untracked files (`engine.rs:1429-1439`), so a
  stale pass cannot carry over.
- **Orchestrator triage** (`run_triage`) cannot approve protected paths or
  spend more budget. It is never called for those questions
  (`engine.rs:3616`, `1859`), and `sanitize_triage` refuses `approve`/`stop`
  anyway. The chat path in G7 is the one that lacks these guards.
- **Commit hooks:** `git::commit` disables repository hooks
  (`core.hooksPath=/dev/null`, `orchd/src/git.rs:305-322`), so an agent
  cannot plant a hook that rewrites the accepted commit.
