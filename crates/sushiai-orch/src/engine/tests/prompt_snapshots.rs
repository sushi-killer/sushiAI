// The agent-turn prompts of the engine, rendered on fixed fixtures and
// compared byte for byte with their goldens in
// `tests/fixtures/prompt-snapshots/engine/<kind>.txt` (the chat module's own
// snapshots sit in chat.rs). A missing golden fails;
// `UPDATE_PROMPT_SNAPSHOTS=1` writes the goldens instead.

use crate::prompts::assert_engine_snapshot as snapshot;

fn snapshot_task() -> Task {
    let mut task = test_support::task_with_status(TaskStatus::Running);
    task.title = "Add a Save button".into();
    task.goal = "Add a Save button to the settings dialog that writes the settings file.".into();
    task.criteria = vec![
        "The settings dialog shows a Save button -- check: `npm test -- settings`".into(),
        "Clicking Save writes settings.json -- check: `npm test -- save`".into(),
    ];
    task.verify = vec!["npm test -- settings".into(), "npm run lint".into()];
    task.final_verify = vec!["npm run build".into()];
    task.paths = vec!["src/settings.ts".into()];
    task.request = Some("Add a Save button to the settings dialog.".into());
    task
}

#[test]
fn prompt_snapshot_impossible_criterion_judge() {
    snapshot(
        "answer_policy_judge",
        &answer_policy::impossible_brief(
            "The settings dialog shows a Save button -- check: `npm test -- settings`",
            "There is no settings dialog in this repository; src/settings.ts does not exist.",
        ),
    );
}

#[test]
fn prompt_snapshot_planner_check_judge() {
    snapshot(
        "answer_policy_planner_check_judge",
        &answer_policy::planner_check_brief(
            "Clicking Save writes settings.json -- check: `npm test -- save`",
            &[
                "node --test tests/save.test.cjs".to_string(),
                "held-out check (criterion 1)".to_string(),
            ],
            "The design writes settings through the store, so tests/save.test.cjs has no save() to call.",
        ),
    );
}

#[test]
fn prompt_snapshot_test_claim_judge() {
    let task = snapshot_task();
    snapshot(
        "test_claim_judge",
        &test_claim::judge_brief(
            &task,
            "tests/save.test.ts",
            "test('save writes settings.json', () => {\n  expect(save()).toBe(true);\n});\n",
            "save() returns nothing by design; the criterion asks for the file, not a return value.",
        ),
    );
}

#[test]
fn prompt_snapshot_test_claim_rewrite() {
    let task = snapshot_task();
    snapshot(
        "test_claim_rewrite",
        &test_claim::rewrite_brief(
            &task,
            "tests/save.test.ts",
            "test('save writes settings.json', () => {\n  expect(save()).toBe(true);\n});\n",
            "save() returns nothing by design; the criterion asks for the file, not a return value.",
        ),
    );
}

#[test]
fn prompt_snapshot_brief_check() {
    let tools = json!({"builtIn": "read files", "mcp": ["asana: read tools none; write tools none"]});
    snapshot(
        "brief_check_check",
        &brief_check::build_check_brief(
            "Add a Save button.",
            &["It shows a Save button".to_string(), "It saves".to_string()],
            &tools,
        ),
    );
}

#[test]
fn prompt_snapshot_brief_check_refresh() {
    let task = snapshot_task();
    let changed = vec!["src/settings.ts".to_string(), "src/save.ts".to_string()];
    snapshot(
        "brief_check_refresh",
        &brief_check::refresh_brief(&task, &changed),
    );
}

#[test]
fn prompt_snapshot_brief_check_redraft_note() {
    snapshot(
        "brief_check_redraft_note",
        &brief_check::redraft_request_note("\"It saves\" and \"It never writes a file\""),
    );
}

#[test]
fn prompt_snapshot_plan_gate_review() {
    let task = snapshot_task();
    let child = brief::PlanSubtask {
        key: "a".into(),
        title: "Save button".into(),
        goal: "Show the button".into(),
        criteria: vec!["It shows -- check: `npm test`".into()],
        verify: vec!["npm test".into()],
        paths: vec!["src/settings.ts".into()],
        ..Default::default()
    };
    snapshot(
        "plan_gate_review",
        &plan_gate::build_review_prompt(&task, "Add a Save button.", &[child]),
    );
}

#[test]
fn prompt_snapshot_plan_gate_findings() {
    let findings = vec![
        PlanFinding {
            id: "f1".into(),
            severity: Severity::Critical,
            kind: "missing requirement".into(),
            text: "Nothing proves the file is written.".into(),
        },
        PlanFinding {
            id: "f2".into(),
            severity: Severity::Low,
            kind: "wrong split".into(),
            text: "Two subtasks edit src/settings.ts.".into(),
        },
    ];
    snapshot("plan_gate_findings", &plan_gate::findings_block(&findings));
}

#[test]
fn prompt_snapshot_healing_feasibility() {
    let task = snapshot_task();
    let failed: VerifyOutcome = serde_json::from_value(json!({
        "command": "npm test -- save",
        "code": 1,
        "tail": "  1 failing\n  save is not a function\n",
        "ms": 10,
    }))
    .unwrap();
    snapshot("healing_feasibility", &healing::feasibility_prompt(&task, &failed));
}

#[test]
fn prompt_snapshot_healing_ruling() {
    let task = snapshot_task();
    let rep = healing::Repeated {
        criterion: 1,
        earlier: (1, "Clicking Save writes nothing yet.".into()),
        current: (2, "No test proves the write.".into()),
    };
    snapshot("healing_ruling", &healing::ruling_prompt(&task, &rep));
}

#[test]
fn prompt_snapshot_healing_answer() {
    let task = snapshot_task();
    let answers = vec![
        "Owner: use the JSON file, not the database".to_string(),
        "Policy: keep going".to_string(),
    ];
    snapshot("healing_answer", &healing::answer_prompt(&task, &answers));
}

#[test]
fn prompt_snapshot_past_work() {
    let task = snapshot_task();
    let notes = vec![RepoNote {
        id: "n1".into(),
        text: "Settings are stored in settings.json, never in the registry.".into(),
        source: "owner".into(),
        created_at: 1_000,
    }];
    snapshot(
        "past_work",
        &past_work::past_work_section(&task, &notes, &[]),
    );
}

#[test]
fn prompt_snapshot_permissions_tools_for_the_plan() {
    snapshot(
        "permissions_tools_plan",
        &permissions::tools_block_text(&tools_fixture().0, &tools_fixture().1, true),
    );
}

#[test]
fn prompt_snapshot_permissions_tools_for_the_implementer() {
    snapshot(
        "permissions_tools_implement",
        &permissions::tools_block_text(&tools_fixture().0, &tools_fixture().1, false),
    );
    snapshot(
        "permissions_tools_none",
        &permissions::tools_block_text(&[], &[], false),
    );
}

fn tools_fixture() -> (Vec<String>, Vec<(String, String)>) {
    (
        vec!["github: read tools list_issues; write tools create_issue (each write asks the owner before it runs)".to_string()],
        vec![("asana".to_string(), "Asana".to_string())],
    )
}

#[test]
fn prompt_snapshot_chat_tools() {
    let listed = vec![
        (
            "github".to_string(),
            vec!["list_issues".to_string(), "get_issue".to_string()],
            vec!["create_issue".to_string()],
        ),
        ("sheets".to_string(), Vec::new(), Vec::new()),
    ];
    snapshot("chat_tools_prompt", &chat_tools::tools_prompt(&listed));
}

#[test]
fn prompt_snapshot_chat_tools_send() {
    snapshot(
        "chat_tools_send",
        &chat_tools::send_prompt(
            "create_issue",
            &json!({"title": "Crash on save", "labels": ["bug"]}),
        ),
    );
}

#[test]
fn prompt_snapshot_decision_memory_precedents() {
    let notes = "\nTask \"Add a Save button\":\n- the owner answered \"retry\" on 2026-01-02 (x2): the base was flaky\n";
    snapshot(
        "decision_memory_precedents",
        &decision_memory::precedents_prompt("Answer the open questions.", notes),
    );
}

#[test]
fn prompt_snapshot_decision_memory_rules() {
    snapshot(
        "decision_memory_rules",
        &decision_memory::rules_prompt(
            "Digest the repo's tasks.",
            "- retry when the base check is flaky\n- never touch generated files\n",
        ),
    );
}
