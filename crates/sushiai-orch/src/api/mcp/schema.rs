use serde_json::{json, Value};

/// What a task agent gets (`--task`): only messaging, always as itself.
pub const TASK_TOOLS: [&str; 4] = [
    // In `tool_specs` order, so a task's tools/list reads the same way.
    "peer_list",
    "peer_send",
    "inbox_read",
    "ask_orchestrator",
];

/// What a Brainstorm or Plan turn gets (`--read-only`): looking, never changing.
pub const READ_ONLY_TOOLS: [&str; 5] = [
    "task_list",
    "task_get",
    "settings_get",
    "repo_notes_list",
    "costs_summary",
];

/// What the orchestrator agent gets: every tool below.
pub const ORCHESTRATOR_TOOLS: [&str; 27] = [
    "task_list",
    "task_get",
    "task_create",
    "task_backlog",
    "task_start",
    "task_report",
    "task_pr",
    "task_lead_touch",
    "task_answer",
    "task_amend",
    "settings_get",
    "task_archive",
    "task_unarchive",
    "task_stop",
    "task_delete",
    "peer_list",
    "peer_send",
    "inbox_read",
    "ask_orchestrator",
    "orchestrator_reply",
    "repo_audit",
    "evolution_run",
    "repo_notes_list",
    "repo_notes_add",
    "repo_notes_remove",
    "costs_summary",
    "hosts_list",
];

/// The tools an orchestrator bridge may point at another saved host with `host`; the call
/// then goes to the daemon's `relay.call`, which the desktop app answers.
pub(super) const HOST_TOOLS: [&str; 13] = [
    "task_list",
    "task_get",
    "task_create",
    "task_backlog",
    "task_start",
    "task_report",
    "task_pr",
    "task_lead_touch",
    "task_answer",
    "task_amend",
    "task_archive",
    "task_unarchive",
    "inbox_read",
];

/// The tools whose `host` may be `all`: every saved host at once.
pub(super) const ALL_HOSTS_TOOLS: [&str; 2] = ["task_list", "inbox_read"];

pub(super) fn host_schema(tool: &str) -> Value {
    let all = if ALL_HOSTS_TOOLS.contains(&tool) {
        ", or \"all\" for every saved host (results labelled with their host, unreachable hosts listed)"
    } else {
        ""
    };
    json!({
        "type": "string",
        "description": format!(
            "Run this on another saved host: \"local\" (This Mac) or \"ssh:<id>\" from hosts_list{all}. Omit for this machine's own orchestrator."
        ),
    })
}

fn from_schema() -> Value {
    json!({
        "type": "string",
        "description": "Sender: a task id, or \"orchestrator\" (the default).",
    })
}

/// name, orchd method, description, JSON Schema for `inputSchema`. No
/// `settings_set` (would let an agent silently change parallelism/planner
/// config the owner didn't ask to change), no `secrets_*` (an agent has no
/// business reading or writing API keys) -- both deliberately left off this
/// surface, not just forgotten. `task_stop` and `task_delete` are the
/// owner's orchestrator only (never `--task` or `--read-only`); the daemon
/// refuses a bridge delete of a live task, and an overseer turn never deletes.
pub(super) fn tool_specs() -> Vec<(&'static str, &'static str, &'static str, Value)> {
    vec![
        (
            "task_list",
            "task.list",
            "List tasks, optionally filtered by repo.",
            json!({
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Absolute path; only list tasks for this repo."},
                    "includeArchived": {"type": "boolean", "description": "Include archived tasks; omitted or false hides them."},
                },
            }),
        ),
        (
            "task_get",
            "task.get",
            "Get one task by id.",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
            }),
        ),
        (
            "task_create",
            "task.create",
            "Create a task. Prefer {repo, request, start: true} and let the planner draft title/goal/criteria/verify; use the full {repo, title, goal, criteria, verify} form only when the owner already specified it.",
            json!({
                "type": "object",
                "properties": {
                    "repo": {"type": "string"},
                    "request": {"type": "string", "description": "One-sentence ask; drafted by the planner."},
                    "title": {"type": "string"},
                    "goal": {"type": "string"},
                    "criteria": {"type": "array", "items": {"type": "string"}},
                    "verify": {"type": "array", "items": {"type": "string"}},
                    "finalVerify": {"type": "array", "items": {"type": "string"}, "description": "Slow checks (full CI, desktop smoke) run once, after review passes."},
                    "screenshot": {"type": "string", "description": "Repo command that captures the screenshot evidence for a UI task; the orchestrator runs it after verify passes and saves the images. The attempt still fails its evidence gate when the images the visual criteria name are missing after it ran. Leave out when the repo has no such command."},
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "Repo-relative files or directories the task edits; it waits while another live task on the same base holds any of them."},
                    "branch": {"type": "string"},
                    "base": {"type": "string", "description": "Branch or commit to start from; defaults to the repo's current HEAD."},
                    "variant": {"type": "object", "description": "Experiment flags for this task only, over the settings defaults: stallTimeoutSecs (0 = off), reviewEvidence, advisor (bool), loopDetect (bool; on by default: stops an attempt that repeats itself), bestOf (2 = on: on the hard tier the first implement attempt runs twice at once in two worktrees, on the tier route and on bestOfRoute, default the first route of the other harness; verify and the checks pick, the other-family reviewer picks when both pass, the loser is removed and both costs count; 0/1 = off); bestOfRoute (route id of the second candidate); plannerRoute (route id the plan stage runs on instead of the settings' planner), judgeRoute (route id for disputed finding judgments; auto by default), and tierRoutes (object tier -> route id, e.g. {\"hard\": \"claude-sonnet\"}, replacing the settings' tier route for implementing; must name configured routes); maxCostUsd (dollar budget; the task waits for the owner before its next run once spent, 0 = none); maxAttemptCostUsd (dollar cap on one implement attempt, stopped mid-run once its streamed usage passes it and retried, 0 = none). Create the same task twice with different variants to A/B them."},
                    "land": {"type": "boolean", "description": "Land the finished top-level task on its base branch by itself (same as variant.land, default true for a top-level task): a per-(repo, branch) queue carries its work onto the branch head, squashes it to one commit, runs verify and finalVerify on that tree, and moves the branch (git merge --ff-only in its clean checkout, else update-ref). Conflicts or failing checks come back as an ordinary failed attempt; a dirty checkout makes the task wait `landing`, retried every 2 minutes and on task.start. Never pushes, and lands on the default branch only when the repo is allowed (settings.landOnDefaultRepos, or settings.landOnDefault for every repo). settings.afterLand [{repo, run}] runs commands in the checkout after a landing."},
                    "checks": {"type": "array", "items": {"type": "object", "properties": {"criterion": {"type": "integer", "description": "0-based index into criteria."}, "run": {"type": "string", "description": "Shell command run from the repo root; must fail before the work and pass after it."}}, "required": ["criterion", "run"]}, "description": "Executable checks per criterion. Entries with an out-of-range criterion or an empty run are dropped."},
                    "heldOut": {"type": "object", "properties": {"criterion": {"type": "integer"}, "run": {"type": "string"}}, "required": ["criterion", "run"], "description": "One extra check the implementer never sees."},
                    "dependsOn": {"type": "array", "items": {"type": "string"}, "description": "Ids of tasks in this repo that must be done before this one starts; it starts on its own once they are. A cycle is rejected."},
                    "parent": {"type": "string", "description": "Id of the task this one is a part of: it branches from the parent's branch and lands there when done. The parent runs no implement attempt of its own; a task that already has a running loop is rejected as a parent."},
                    "start": {"type": "boolean", "description": "Start right away (default true); false leaves the task stopped for review."},
                    "mcp": {"type": "object", "properties": {"mcpServers": {"type": "object"}}, "description": "MCP servers for this task's runs, {\"mcpServers\": {name: {command, args, env} | {url}}}. Claude runs get them with every write asking the owner. A Codex run gets only a server marked \"codex\": true, and nothing gates its writes, so mark only a server the task may use freely and say in the goal what it must not change."},
                    "backlog": {"type": "object", "properties": {"bucket": {"type": "string", "enum": ["next", "later"]}, "order": {"type": "integer"}}, "required": ["bucket"], "description": "Park the task in the plan backlog instead of starting it. Not for a subtask: its parent's place decides."},
                },
                "required": ["repo"],
            }),
        ),
        (
            "task_backlog",
            "task.backlog",
            "Move an unstarted top-level task into the plan backlog's next or later bucket, or out of the backlog with bucket null. order places it within the bucket (ascending); omitted, it goes last.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "bucket": {"type": ["string", "null"], "enum": ["next", "later", null]},
                    "order": {"type": "integer"},
                },
                "required": ["id", "bucket"],
            }),
        ),
        (
            "task_start",
            "task.start",
            "Start (or resume) a queued, stopped, failed or drafting task.",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
            }),
        ),
        (
            "task_report",
            "task.report",
            "The report of a finished top-level task or graph, as markdown: outcome, what changed, each criterion's status, assumptions and automatic answers, cost by stage and model, attempts, follow-ups and its lead-touch mark. Returns {id, report}; errors while the task is not done.",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
            }),
        ),
        (
            "task_pr",
            "task.pr",
            "Open a pull request for a done top-level task: pushes the task's branch and opens the PR on the repo's remote. Use it when a task is done and green. Never merge: merging is the owner's.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "title": {"type": "string", "description": "PR title; defaults to the task title."},
                },
                "required": ["id"],
            }),
        ),
        (
            "task_lead_touch",
            "task.leadTouch",
            "Mark whether a done task's work needed a fix from a person or the lead session after the orchestrator said done (touched true) or was clean (touched false); omit touched to clear the mark. An owner's touched mark with a non-empty note also creates one follow-up task from that note (the same note never creates a second one). The autonomy metric behind costs.summary's leadTouch rate.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "touched": {"type": "boolean"},
                    "note": {"type": "string", "description": "What had to be fixed."},
                },
                "required": ["id"],
            }),
        ),
        (
            "task_answer",
            "task.answer",
            "Answer a task's pending question (send \"stop\" as the answer to cancel it instead). When an owner precedent or a repo note covers it, pass `like` naming it.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "answer": {"type": "string"},
                    "like": {"type": "string", "description": "Only when an owner precedent or a repo note covers the question: name it (one line, at most 120 characters). Refused for questions the owner alone decides."},
                },
                "required": ["id", "answer"],
            }),
        ),
        (
            "task_amend",
            "task.amend",
            "Replace a drafting, running, waiting, queued or stopped task's criteria, verify, finalVerify, screenshot, checks, heldOut or dependsOn (each field you pass replaces the task's; at least one is required). dependsOn replaces the full list of task ids only before the first implement attempt; an empty list makes the task ready. A live loop applies it at its next attempt boundary, never mid-run. The task gains an `Amended: <field names>` decision line. Amended checks and heldOut are re-run on the base commit and only those failing there gate.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "criteria": {"type": "array", "items": {"oneOf": [{"type": "string"}, {"type": "object", "properties": {"text": {"type": "string"}, "visual": {"type": "boolean", "description": "True only when the criterion is proven by looking at a saved image; never for a criterion checked by a command."}}, "required": ["text"]}]}},
                    "verify": {"type": "array", "items": {"type": "string"}},
                    "finalVerify": {"type": "array", "items": {"type": "string"}, "description": "Slow checks run once, after review passes."},
                    "screenshot": {"type": "string", "description": "Replaces the repo command that captures the screenshot evidence; an empty string removes it."},
                    "checks": {"type": "array", "items": {"type": "object", "properties": {"criterion": {"type": "integer", "description": "0-based index into criteria."}, "run": {"type": "string"}}, "required": ["criterion", "run"]}, "description": "Replaces the task's checks; a criterion index out of range is rejected."},
                    "heldOut": {"type": ["object", "null"], "properties": {"criterion": {"type": "integer"}, "run": {"type": "string"}}, "required": ["criterion", "run"], "description": "Replaces the held-out check; null removes it."},
                    "dependsOn": {"type": "array", "items": {"type": "string"}, "description": "The full new list of task ids; only before the first implement attempt; an empty list makes the task ready."},
                },
                "required": ["id"],
            }),
        ),
        (
            "settings_get",
            "settings.get",
            "Read the orchestrator settings (parallelism, planner, profiles).",
            json!({"type": "object", "properties": {}}),
        ),
        (
            "task_archive",
            "task.archive",
            "Archive a task, hiding it from the default task_list without deleting it. Refused for a running, drafting, or waiting task.",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
            }),
        ),
        (
            "task_unarchive",
            "task.unarchive",
            "Restore an archived task so it appears in the default task_list again.",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
            }),
        ),
        (
            "task_stop",
            "task.stop",
            "Stop a running, drafting, waiting or landing task. Use it when a live task must end; the task can be started again later.",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
            }),
        ),
        (
            "task_delete",
            "task.delete",
            "Delete a task with its worktree and branch. Refused while the task is running, drafting, waiting or landing: task_stop it first. Only when the owner asks.",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
            }),
        ),
        (
            "peer_list",
            "peer.list",
            "List the other tasks in a repository (id, title, status) you can message.",
            json!({
                "type": "object",
                "properties": {
                    "from": from_schema(),
                    "repo": {"type": "string", "description": "Absolute path; needed only when sending as the orchestrator."},
                },
            }),
        ),
        (
            "peer_send",
            "message.send",
            "Message another task by id. It reads the message on its next attempt; the attempt it is running is not interrupted.",
            json!({
                "type": "object",
                "properties": {
                    "from": from_schema(),
                    "to": {"type": "string", "description": "The recipient task's id."},
                    "text": {"type": "string"},
                    "replyTo": {"type": "string", "description": "Id of a message sent to you that this answers."},
                },
                "required": ["to", "text"],
            }),
        ),
        (
            "inbox_read",
            "message.inbox",
            "Read every message sent to you, delivered or still waiting for your next turn.",
            json!({"type": "object", "properties": {"from": from_schema()}}),
        ),
        (
            "ask_orchestrator",
            "message.send",
            "Ask the orchestrator a question without stopping your work. Its reply arrives with your next attempt.",
            json!({
                "type": "object",
                "properties": {"from": from_schema(), "text": {"type": "string"}},
                "required": ["text"],
            }),
        ),
        (
            "orchestrator_reply",
            "message.send",
            "Answer a question a task asked you; the task reads it on its next attempt.",
            json!({
                "type": "object",
                "properties": {
                    "question": {"type": "string", "description": "The question's message id."},
                    "text": {"type": "string"},
                },
                "required": ["question", "text"],
            }),
        ),
        (
            "repo_audit",
            "repo.audit",
            "Start a read-only audit of a repository against a fixed agent-readiness rubric (instructions, build/test commands, test health, legibility, context hygiene, safety, verification surfaces). Returns the audit record with its id at once; the run continues in the background and stores a graded report with file-referenced recommendations.",
            json!({
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Absolute path of the repository to audit."},
                    "route": {"type": "string", "description": "Route id to run it on; defaults to the planner's route."},
                },
                "required": ["repo"],
            }),
        ),
        (
            "evolution_run",
            "evolution.run",
            "Cluster the recorded task signals (repeated waste: loops, throwaway scripts, repeated owner questions, ...) and start a read-only proposer run for each cluster that meets the evidence thresholds and has no proposal yet, at most the configured maximum. Returns the started clusters at once; each proposal is stored when its run ends. Also updates the measurement of adopted proposals.",
            json!({"type": "object", "properties": {}}),
        ),
        (
            "repo_notes_list",
            "repo.notes.list",
            "List the owner's standing notes for a repository (id, text, source, createdAt). They are shown to the planner in every plan brief.",
            json!({
                "type": "object",
                "properties": {"repo": {"type": "string", "description": "Absolute path of the repository, or any directory in it."}},
                "required": ["repo"],
            }),
        ),
        (
            "repo_notes_add",
            "repo.notes.add",
            "Add a standing note for a repository. Only when the owner asks for it.",
            json!({
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Absolute path of the repository, or any directory in it."},
                    "text": {"type": "string"},
                },
                "required": ["repo", "text"],
            }),
        ),
        (
            "repo_notes_remove",
            "repo.notes.remove",
            "Remove a repository note by id. Only when the owner asks for it.",
            json!({
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Absolute path of the repository, or any directory in it."},
                    "id": {"type": "string"},
                },
                "required": ["repo", "id"],
            }),
        ),
        (
            "costs_summary",
            "costs.summary",
            "Spend and usage totals, read-only: by day, model, stage and task, with the lead-touch rate. Filter by repo, taskId, sinceDays or a from/to day (YYYY-MM-DD); groupBy adds breakdowns.",
            json!({
                "type": "object",
                "properties": {
                    "repo": {"type": "string"},
                    "taskId": {"type": "string"},
                    "sinceDays": {"type": "integer"},
                    "from": {"type": "string"},
                    "to": {"type": "string"},
                    "groupBy": {"type": "array", "items": {"type": "string"}},
                },
            }),
        ),
        (
            "hosts_list",
            "hosts.list",
            "List the hosts whose orchestrator you can reach with a tool's `host` argument: This Mac (\"local\") and every saved ssh host (\"ssh:<id>\"), each with its name and state (ready, connecting, idle, error). Needs the sushiai desktop app running.",
            json!({"type": "object", "properties": {}}),
        ),
    ]
}
