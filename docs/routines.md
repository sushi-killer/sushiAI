# Routines

A routine is a task the owner describes once in a sentence and an agent then
runs on a schedule, in the background, on a host the owner picks.

## What a routine is

A routine is a folder on its host, `~/.sushiai/routines/<id>/`:

- `routine.json`: where it runs (host and zero to 20 projects), when (cron and time
  zone), which agent, how it runs, and limits.
- `brief.md`: what each run does. It is the only source of instructions for
  the run.

## Build one

1. Open Routines and type one sentence, for example "Every weekday at 9
   summarize new issues of my-app".
2. A builder session (Claude Code or Codex, chosen in Settings, Routines)
   reads your projects and hosts, picks a project and a host, and writes the
   brief and the routine. It uses the `sushiai-routines` skill and follows
   `sushiai routines help`.
   Both builders run headless, one turn per message (`headless` on
   `session.create`: `claude -p`, `codex exec`): no folder trust prompt, each
   turn works in the build's own folder, and your next message resumes the same
   conversation. A turn cannot wait for an answer, so the builder ends it with
   its question and never uses `sushiai ask`. The Codex builder runs with the
   `workspace-write` sandbox, no network access and no `/tmp` or `$TMPDIR`
   write access: it writes only its own folder. It ignores the MCP servers of
   your Codex config; the sushiai MCP server is the only one it gets. The
   sandbox cannot reach the daemon socket, so a Codex builder always uses
   `mcp` access.
   The Claude builder may run only `sushiai routines help|show`, `projects`,
   `hosts`, `routine validate|save|dry-run`, and read and edit inside its folder.
3. The builder checks the routine, saves it as a draft (not enabled) and runs
   a dry run. It shows what the routine would do.
4. You read the dry run, grant the access it proposes and turn the routine on.

In Settings, Routines you choose what the builder sees (projects, connections)
and how it reaches sushiai: the commands (`cli`), the MCP server (`mcp`) or
`both` (Codex: `mcp` only). With `mcp` or `both` the builder session gets the server
`sushiai mcp --routines`: `routines_projects`, `routines_hosts`,
`routines_list`, `routines_show`, `routines_validate`, `routines_save` and
`routines_dry_run`, with the same checks and host rules as the commands.
`ask` and `propose` are not tools: only a run has them.

The builder never edits your project files and asks you (in its reply) before any write
outside the routine folder.

## Dry run

A dry run is one run that writes nothing. Git changes and outside writes are
listed as actions with `applied` false, and `sushiai ask` is skipped. A dry run
uses only tools you granted and that are read-only (access `kind` `read`), so a
routine that reads a chat or a tracker through connector tools can be dry-run
after you grant those reads. Write tools and tools you did not grant stay
unavailable; the result names the ones the run needed. Use it to check a brief
before the first real run.

## How it runs

- The daemon of the routine's host starts a background agent session on the
  cron. The session does not show in the sidebar.
- A run still going when the next one is due makes the next one skip. One
  missed run is made up after downtime (within 7 days).
- Every run of a routine with a project works in the routine's own git
  worktree of that project (a clone when the host has no copy). It is reset to
  the default branch head before each run. Your checkout is never touched.
  Scheduled and retry Claude runs use `--permission-mode dontAsk`: a tool the
  routine did not allow is refused, whatever your default mode is.
- A routine with two or more projects (at most 20, Claude only) works in its
  own `work/` folder and gets read access to each project's folder on the host
  (`--add-dir`); a project with no folder there is cloned into
  `projects/<id>` and reset before each run. It is read-only for now:
  `sushiai propose` is refused. Validate refuses Codex with two or more
  projects, because Codex has no extra readable folders to give it. An old
  `where.project` string loads as a one-item `projects` list.
- A dry run may read files, run `git status`, use the granted read-only
  access entries, and call `sushiai ask` and
  `sushiai propose pr`, `commit` and `branch`. It reads history through the worktree's files only;
  `git log`, `diff` and `show` are not allowed because they can write a file.
- Git writes go through `sushiai propose pr`, `sushiai propose commit` and
  `sushiai propose branch`. The daemon applies them after the run, on a
  branch named `routine/<id>/<run>`.
- Outside writes (message, ticket) wait for your answer to `sushiai ask`
  while the routine is in "ask first" mode. See "Run modes".

## Access

A routine lists the tools it needs beyond reading files in `routine.json`.
The builder proposes them, you grant each one in the app. A run may use only
the granted tools, plus the read tools and `sushiai ask` and `sushiai propose`.

```json
"access": [
  {
    "tool": "mcp__tracker__comment",
    "label": "Comment on issues",
    "detail": "Posts the digest as a comment",
    "kind": "write",
    "granted": false
  }
]
```

- `tool` is an exact tool name, `Bash(<command prefix>:*)` with a command that
  is not an interpreter or launcher (`sh`, `env`, `timeout`, `npx`, `python`,
  `osascript`, `curl`, `ssh` and the like; the command word is compared by its
  base name, so `/bin/sh` is `sh`), `Edit(<path>)` with a
  path scope, or one exact `mcp__<server>__<tool>`. Bare `Write`, `Edit`, `Write(<path>)`,
  `MultiEdit`, `NotebookEdit` and `Bash`, `git config`, `git -c`, `gh api` and
  `gh auth` are refused, and so is every other wildcard.
- Codex runs cannot be granted MCP tools yet; use Claude. A Codex run starts
  with an empty MCP server list, a dry run too.
- `kind` is `read` or `write`. `detail` is optional.
- `granted` is yours. A save by the builder resets it for a new or changed
  entry and never sets it. A routine with ungranted access runs with the
  granted tools only and ends `needs_you` ("needs access to ..."); that is not a
  failure and the routine stays on.
- Claude gets the granted tools as its `--allowedTools`. Codex gets the same
  list in its prompt and keeps its sandbox. A dry run uses only granted
  `read` entries and never makes a grant. Codex runs and dry runs start with `mcp_servers={}`, so the
  owner's MCP servers are not reachable.
- A routine's `extraArgs` `--allowedTools` is ignored; use `access`.

The daemon methods are `routines.grant {id, tools: [..] | all: true}` and
`routines.revoke {id, tools}`. `routines.list` and `routines.show` return
`access` and `needsAccess` (the number of ungranted entries).

## Security boundary

The daemon socket is mode 0600: whoever can open it is the owner. `routines.grant`,
`routines.approve` and `routines.revoke` over that socket have no owner card of
their own, so a run must not reach the socket. It does not:

- A run reaches only the file ask channel. Inside a routine run
  (`SUSHIAI_ROUTINE_DIR` set) `sushiai ask` always writes
  `work/asks/<id>.req`; the module forwards it to the owner while the run is
  live and writes `<id>.ans`. Codex runs have no network access, so they cannot
  open the socket. The socket path of `sushiai ask` exists only outside a
  routine.
- Grants, approvals and the owner's "yes" are daemon-owned. The "yes" of an ask
  is recorded by the module alone, in `state/asks/<session>/` of the routine
  folder, after a real owner allow. A run writes only `work/` (and the project
  worktree where it has one), never the routine folder root or `state/`.
- `--add-dir` in `extraArgs` may name only a folder under the routine's
  `work/`.
- On every load the daemon checks the routine: its id matches the folder, the
  mode is known, every access tool and `extraArgs` pass the same checks as
  `validate`, and the hash of `routine.json` plus `brief.md` equals the hash
  recorded at the last save in `state/saved.json`. A routine that fails is
  unusable: its run is refused and the routine is paused with the reason
  "routine changed outside the app; re-save it". Only a save from the app
  makes it usable again, as a draft with no grant.
- MCP tools are not available to Codex runs.

## Run modes

`howItRuns` is `{"mode", "promoteAfter": 5}`.

- `ask-every-time`: the brief says to ask before every write, and a
  `sushiai propose` is not applied until you answer. A "yes" of the same kind
  to `sushiai ask --kind K` applies it; so does `routines.approve {id, run,
proposal}`. Until then the proposal has status `pending` and its files are
  kept with the run. A failed apply keeps it `pending` with `lastError`, so
  you can approve again.
- `ask-first`: asks like the mode above, so a proposal of a kind that is not
  on its own waits as `pending` unless you answered a "yes" of that kind. Each
  "yes" counts for the kind of the write (`--kind`, for example `pr`,
  `comment`, `message`, `ticket`). After `promoteAfter` accepted asks of one
  kind, that kind runs on its own.
- `on-its-own`: never asks.

The counts are kept next to the routine, not in `routine.json`, so counting
never turns a reviewed routine back into a draft. `routines.show` returns
`howItRuns.accepted` (the most accepted asks of a kind that still asks),
`acceptedByKind` and `onItsOwn` (the kinds that no longer ask).

## Skills

`routine.json` may list `skills`: zero to ten names (letters, digits and
`:_.-`, at most 64, no duplicates; a plugin skill is `plugin:skill`). The brief
can then stay short and point to the skill. `sushiai skills [--project DIR]`
(MCP: `routines_skills`) lists what the machine offers: name, harness (`claude`
or `codex`, from the one table of skill folders in `sushiai_agents::skills`,
with the project's own), path, scripts and the first line of the description.
A file linked into several folders of one harness is listed once, and a
disabled skill is not listed. It never prints more of a file. Plugin skills are
not listed; name one by hand.

- The run prompt tells the agent to use each listed skill first: `/name` on
  Claude, `$name` on Codex. A brief may also start with its own `/name args`
  line; it is left as written. The names come from one function, `skill_call`
  in `argv.rs`.
- Claude cannot limit a run to a list of skills; it can only deny. A routine
  with `skills` leaves the `Skill` tool on, so the agent can call any installed
  skill. A routine without `skills` gets `--disallowedTools Skill` (merged with
  its own `--disallowedTools`). A Claude dry run has a fixed tool set; `Skill` is
  added to it only when `skills` is not empty.
- Codex finds skills in `CODEX_HOME/skills` even though the run ignores the
  user config.
- A skill script runs through Bash, which is allowlist-gated, so it needs an
  `access` entry the owner grants. Besides a plain path
  (`Bash(/abs/skills/<name>/scripts/run.sh:*)`), `Bash(<interp> <path>:*)`
  is accepted when `<interp>` is `python3`, `python`, `node`, `bash` or `sh`,
  there is exactly one absolute path, it has no `..` or shell character, and
  it contains `skills/<name>/<file>` with `<name>` in `skills`. Every other use
  of an interpreter (`python3 -c`, `python3:*`, a path outside a skills folder
  or of a skill the routine does not list) stays refused. A run that needs a
  script without a grant ends `needs_you`.
- `skills` is part of the saved hash, like the rest of `routine.json`.

## Login problems

A run that failed because the harness is not logged in records `loginHost` and
`loginHarness` in its run. The app opens a plain shell session on that host that
runs `claude /login` or `codex login` through a login shell, so a harness
installed under `~/.local/bin` is found.

## Agent per routine

Every routine has its own harness (Claude Code or Codex), model and thinking
level (low, medium or high). A new routine starts with the Standard route from
Settings, Orchestration. The builder may propose another and says why. A model
the routine's host lacks is not offered.

## Result

A run ends with one JSON object:

```json
{
  "status": "done",
  "title": "3 new issues",
  "summary": "markdown, up to 64 KB",
  "actions": [{ "kind": "comment", "text": "Triaged #12", "applied": true }],
  "missing": [],
  "next": "",
  "counts": { "issues": 3 }
}
```

`status` is `done`, `noop`, `needs_you`, `failed` or `missing_tool`. `noop`
means nothing to do and is archived quietly. `needs_you` and `failed` reach
the Inbox. An invalid answer gets one retry, then the run is failed.

## Limits

- Cron has five fields and fires at most every 5 minutes. The time zone is an
  IANA name.
- The last 30 runs of a routine are kept.
- Three failed runs in a row pause the routine.
- `extraArgs` is an allowlist: no `--dangerously-skip-permissions`,
  `--add-dir` only inside the routine's `work/` folder, no sandbox or approval
  overrides.
- A run has the owner's rights on its host. Outside writes are consent
  through `sushiai ask`, not a sandbox.
- Codex runs use the workspace-write sandbox without network access and with
  an empty MCP server list. Their asks go through the file channel.
- Old saved shell-command routines are removed and not migrated.

## Command reference

Run `sushiai routines help`. It is the single source of the command list and
matches the CLI on the host.
