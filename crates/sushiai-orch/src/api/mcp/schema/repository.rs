use super::ToolSpec;
use serde_json::json;

pub(super) fn settings() -> Vec<ToolSpec> {
    vec![(
        "settings_get",
        "settings.get",
        "Read the orchestrator settings (parallelism, planner, profiles).",
        json!({"type": "object", "properties": {}}),
    )]
}

pub(super) fn analysis() -> Vec<ToolSpec> {
    vec![
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
    ]
}

pub(super) fn notes() -> Vec<ToolSpec> {
    vec![
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
    ]
}

pub(super) fn costs() -> Vec<ToolSpec> {
    vec![
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
    ]
}

pub(super) fn hosts() -> Vec<ToolSpec> {
    vec![
        (
    "hosts_list",
    "hosts.list",
    "List the hosts whose orchestrator you can reach with a tool's `host` argument: This Mac (\"local\") and every saved ssh host (\"ssh:<id>\"), each with its name and state (ready, connecting, idle, error). Needs the sushiai desktop app running.",
    json!({"type": "object", "properties": {}}),
),
    ]
}
