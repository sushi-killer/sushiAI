//! Every task-agent brief, rendered through its canonical owner on fixed fixtures and
//! compared byte for byte with its golden in
//! `tests/fixtures/prompt-snapshots/briefs/<kind>.txt`. A missing golden
//! fails; `UPDATE_PROMPT_SNAPSHOTS=1` writes the goldens instead.
//!
//! Uses public builders and model types. The fixtures hold no clock and no temp path.

use std::path::PathBuf;

use serde_json::{json, Value};
use sushiai_orch::engine::attempts::pick::PickCandidate;
use sushiai_orch::engine::evolution::proposal::ProposalBriefInput;
use sushiai_orch::engine::evolution::proposal::ProposalExcerpt;
use sushiai_orch::policy::settings::ScopedCheck;
use sushiai_orch::policy::variant::Variant;
use sushiai_orch::task::messages::Message;
use sushiai_orch::task::{Decline, DeclineReason, Task, TaskStatus, VerifyOutcome};
use sushiai_orch::verify::review::brief::NarrowReview;
use sushiai_orch::verify::review::routing::Lenses;

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/prompt-snapshots/briefs")
}

/// `fields` over a minimal task.
fn task(fields: Value) -> Task {
    let mut base = json!({
        "id": "t-fixture",
        "title": "Add a Save button",
        "goal": "Add a Save button to the settings dialog that writes the settings file.",
        "criteria": [
            "The settings dialog shows a Save button -- check: `npm test -- settings`",
            "Clicking Save writes settings.json -- check: `npm test -- save`"
        ],
        "verify": ["npm test -- settings", "npm run lint"],
        "repo": "/repo",
        "worktree": "/repo-task",
        "branch": "task/add-a-save-button",
        "baseSha": "abc123",
        "status": "running",
        "tier": "standard",
        "createdAt": 1_000,
        "updatedAt": 2_000
    });
    for (key, value) in fields.as_object().expect("fields are an object") {
        base[key] = value.clone();
    }
    serde_json::from_value(base).expect("fixture task deserializes")
}

fn verify(command: &str, code: Option<i32>, tail: &str) -> Value {
    json!({ "command": command, "code": code, "tail": tail, "ms": 1200 })
}

fn attempt(n: u32, stage: &str, status: &str, fields: Value) -> Value {
    let mut a = json!({
        "n": n,
        "stage": stage,
        "routeId": "claude-opus",
        "harness": "claude",
        "model": "opus",
        "reason": "tier standard",
        "startedAt": 10_000 * n as i64,
        "endedAt": 10_000 * n as i64 + 5_000,
        "status": status
    });
    for (key, value) in fields.as_object().expect("fields are an object") {
        a[key] = value.clone();
    }
    a
}

/// The attempts behind every optional section of an implement brief: a plan
/// attempt, a loop that left a handoff, then a verify failure and a review
/// failure (so the disputes block shows).
fn attempts() -> Value {
    json!([
        attempt(1, "plan", "passed", json!({})),
        attempt(
            2,
            "implement",
            "failed",
            json!({
                "handoff": "Added the button; the save handler is still a stub.",
                "failure": {
                    "kind": "loop",
                    "detail": "The same edit to src/settings.ts was repeated 5 times.",
                    "signature": "loop-1"
                }
            })
        ),
        attempt(
            3,
            "implement",
            "failed",
            json!({
                "summary": "Wired the save handler.",
                "verify": [
                    verify("npm test -- settings", Some(0), "ok"),
                    verify("npm run lint", Some(1), "src/settings.ts:12 no-unused-vars"),
                    verify("held-out check (criterion 1)", Some(1), "hidden")
                ],
                "failure": {
                    "kind": "verify",
                    "detail": "`npm test -- save` exited 1.\nexpected settings.json to exist",
                    "signature": "verify-1"
                }
            })
        ),
        attempt(
            4,
            "implement",
            "failed",
            json!({
                "handoff": "Save writes the file; review disagreed about the button label.",
                "review": {
                    "verdict": "FAIL",
                    "findings": [
                        "P1: src/settings.ts:40 - Save ignores write errors",
                        "P2: src/dialog.tsx:12 - the label says Store, not Save"
                    ]
                },
                "failure": {
                    "kind": "review",
                    "detail": "Review failed: P1: src/settings.ts:40 - Save ignores write errors",
                    "signature": "review-1"
                }
            })
        )
    ])
}

/// A task with every optional section of the implement brief filled in.
fn full_task() -> Task {
    task(json!({
        "finalVerify": ["npm run ci"],
        "checks": [
            { "criterion": 0, "run": "npm test -- settings", "baseline": "fail" },
            { "criterion": 1, "run": "npm test -- save", "baseline": "pass" }
        ],
        "heldOut": { "criterion": 1, "run": "npm test -- save-hidden", "baseline": "fail" },
        "plannerTests": {
            "protected": { "tests/settings.test.ts": "sha-1", "tests/save.test.ts": "sha-2" },
            "discarded": ["tests/save.test.ts"]
        },
        "checklist": [
            { "text": "Add the button", "check": "npm test -- button", "passes": true },
            { "text": "Wire the handler", "check": "", "passes": false }
        ],
        "planGate": {
            "findings": [
                { "id": "g1", "severity": "MEDIUM", "kind": "scope", "text": "The dialog also has a Cancel button." }
            ]
        },
        "decisions": [
            "Owner: use the primary button style",
            "Agent: kept the existing settings schema"
        ],
        "assumptions": [
            {
                "question": "Which file holds the settings?",
                "answer": "settings.json",
                "evidence": "src/settings.ts:3 reads it",
                "by": "planner",
                "overturned": false
            },
            {
                "question": "Should Save close the dialog?",
                "answer": "no",
                "evidence": "no other dialog closes on save",
                "by": "planner",
                "overturned": true,
                "ownerAnswer": "yes, close it"
            }
        ],
        "questionHistory": [
            {
                "question": "Keep the old label?",
                "options": ["yes", "no"],
                "kind": "agent_question",
                "answer": "no",
                "answeredAt": 45_000,
                "answeredBy": "owner"
            },
            {
                "question": "An earlier question",
                "options": [],
                "kind": "agent_question",
                "answer": "skipped: answered before the attempt",
                "answeredAt": 1_000,
                "answeredBy": "owner"
            }
        ],
        "briefCheck": {
            "conflict": "criterion [0] wants the button hidden, criterion [1] wants it clicked",
            "check": "npm test -- settings"
        },
        "findings": [
            {
                "id": "F-1a2b3c",
                "text": "P1: src/settings.ts:40 - Save ignores write errors",
                "status": "open",
                "attempt": 4,
                "round": 1
            },
            {
                "id": "F-4d5e6f",
                "text": "P2: src/dialog.tsx:12 - the label says Store, not Save",
                "status": "resolved",
                "attempt": 4,
                "round": 1
            }
        ],
        "report": "## Done\n\nThe Save button writes settings.json.",
        "attempts": attempts()
    }))
}

/// A done dependency with a passed implement attempt.
fn dependency(id: &str) -> Task {
    task(json!({
        "id": id,
        "title": format!("Dependency {id}"),
        "status": "done",
        "decisions": [
            "Agent: stored settings as JSON",
            "Orchestrator: not shown",
            "Owner: keep the file name"
        ],
        "attempts": [attempt(1, "implement", "passed", json!({
            "summary": "Added the settings store.",
            "handoff": "The store lives in src/store.ts.",
            "changedFiles": ["src/store.ts", "tests/store.test.ts"]
        }))]
    }))
}

fn message(id: &str, kind: &str, text: &str) -> Message {
    serde_json::from_value(json!({
        "id": id,
        "repo": "/repo",
        "from": "t-other",
        "to": "t-fixture",
        "kind": kind,
        "text": text,
        "ts": 5_000
    }))
    .expect("fixture message deserializes")
}

fn outcome(command: &str, code: Option<i32>, tail: &str) -> VerifyOutcome {
    serde_json::from_value(verify(command, code, tail)).expect("fixture verify deserializes")
}

fn batch() -> Variant {
    Variant {
        batch_questions: true,
        ..Variant::default()
    }
}

const REQUEST: &str =
    "  Add a Save button to the settings dialog.\n\n1. It writes settings.json.\n2. It shows an error when the write fails.\n";
const PAST_WORK: &str = "## Past work\n\n- t-old: added the settings dialog\n\n";

fn review_verify() -> Vec<VerifyOutcome> {
    vec![
        outcome("npm test -- settings", Some(0), "  12 passing\n"),
        outcome("npm run lint", Some(1), "src/settings.ts:12 no-unused-vars"),
        outcome("held-out check (criterion 1)", Some(0), "hidden"),
    ]
}

/// A review of the latest attempt: the one before it reported findings.
fn review_task() -> Task {
    let mut t = serde_json::to_value(full_task()).unwrap();
    let mut a = t["attempts"][3].clone();
    a["n"] = json!(5);
    a["status"] = json!("running");
    a["review"] = Value::Null;
    a["failure"] = Value::Null;
    t["attempts"].as_array_mut().unwrap().push(a);
    serde_json::from_value(t).unwrap()
}

fn where_to_look() -> String {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src/sub")).unwrap();
    std::fs::write(
        dir.path().join("src/settings.ts"),
        "export function save() {\n  // writes settings.json\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("src/dialog.tsx"),
        "export const Dialog = 1;\n",
    )
    .unwrap();
    let paths = [
        "src/settings.ts",
        "src",
        "missing.ts",
        "../outside.ts",
        "/etc/passwd",
    ]
    .map(String::from);
    sushiai_orch::engine::attempts::context::where_to_look_block(dir.path(), &paths)
}

fn renders() -> Vec<(&'static str, String)> {
    let bare = task(json!({ "criteria": [], "verify": [] }));
    let full = full_task();
    let status = " M src/settings.ts\n?? tests/save.test.ts\n";
    let stat = " src/settings.ts | 4 ++--\n 1 file changed, 2 insertions(+), 2 deletions(-)\n";
    let diff = "--- a/src/settings.ts\n+++ b/src/settings.ts\n@@ -1 +1 @@\n-old\n+new";
    let advice = "The lint failure is an unused import; remove it.";

    let mut pick_a = review_verify();
    pick_a.truncate(1);
    let pick_b = review_verify();

    let mut with_deps = serde_json::to_value(&full).unwrap();
    with_deps["dependsOn"] = json!(["t-dep-a", "t-missing", "t-dep-b"]);
    let with_deps: Task = serde_json::from_value(with_deps).unwrap();
    let mut not_done = serde_json::to_value(dependency("t-dep-b")).unwrap();
    not_done["status"] = json!(TaskStatus::Running);
    let not_done: Task = serde_json::from_value(not_done).unwrap();
    let all = vec![dependency("t-dep-a"), not_done];

    let mut relayed = serde_json::to_value(&full).unwrap();
    relayed["relayOf"] = json!("t-dep-a");
    let relayed: Task = serde_json::from_value(relayed).unwrap();

    let scoped: Vec<ScopedCheck> = vec![
        serde_json::from_value(json!({
            "command": "cargo test -p sushiai-orch",
            "paths": ["crates/sushiai-orch/**", "Cargo.lock"]
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "repo": "/other",
            "command": "npm run e2e",
            "paths": ["e2e/**"]
        }))
        .unwrap(),
        serde_json::from_value(json!({ "command": "npm run unscoped", "paths": [] })).unwrap(),
    ];

    let excerpts = [
        ProposalExcerpt {
            task_id: "t-a".into(),
            file: "events.jsonl".into(),
            from_line: 12,
            lines: vec![
                r#"{"type":"tool","name":"Bash","input":"npm test"}"#.into(),
                "a line with ``` inside".into(),
            ],
        },
        ProposalExcerpt {
            task_id: "t-b".into(),
            file: "events.jsonl".into(),
            from_line: 3,
            lines: vec![r#"{"type":"tool","name":"Bash","input":"npm test"}"#.into()],
        },
    ];
    let proposal = ProposalBriefInput {
        kind: "repeated_full_suite",
        detail: "npm test ran the whole suite",
        repo: "/repo",
        occurrences: 4,
        tasks: 2,
        wasted_calls: 7,
        wasted_usd: 1.5,
        excerpts: &excerpts,
    };
    let proposal_empty = ProposalBriefInput {
        excerpts: &[],
        ..proposal
    };

    let note = "Outcome complete: wired the Save button.";
    let agent_decisions = vec!["Agent: kept the existing settings schema".to_string()];
    let screenshots = vec![
        ("artifacts/settings.png".to_string(), Some(4)),
        ("artifacts/dialog.png".to_string(), None),
    ];
    let review = review_task();
    let first_review = task(json!({}));
    let open_criteria = vec![(1, full.criteria[1].clone())];
    let settled_criteria = vec![(0, full.criteria[0].clone())];
    let narrow = NarrowReview {
        open_criteria: &open_criteria,
        settled_criteria: &settled_criteria,
        delta: diff,
    };
    let narrow_settled = NarrowReview {
        open_criteria: &[],
        settled_criteria: &settled_criteria,
        delta: diff,
    };
    let declines = vec![
        Decline {
            id: "F-1a2b3c".into(),
            reason: DeclineReason::AlreadySatisfied,
            evidence: vec!["src/settings.ts:41 catches the write error".into()],
        },
        Decline {
            id: "F-4d5e6f".into(),
            reason: DeclineReason::Wrong,
            evidence: vec!["src/dialog.tsx:12".into(), "artifacts/dialog.png".into()],
        },
    ];
    let some_lenses = Lenses {
        seams: true,
        code: false,
        tests: true,
        errors: false,
        docs: true,
    };
    let no_lenses = Lenses {
        seams: false,
        code: false,
        tests: false,
        errors: false,
        docs: false,
    };
    let changed = vec!["src/settings.ts".to_string(), "src/dialog.tsx".to_string()];
    let passed = full.clone();
    let mut last = serde_json::to_value(&passed.attempts[2]).unwrap();
    last["n"] = json!(5);
    last["status"] = json!("passed");
    last["failure"] = Value::Null;
    last["verify"] = json!([
        verify("npm test -- settings", Some(0), "ok"),
        verify("npm run lint", None, "killed")
    ]);
    let mut passed_json = serde_json::to_value(&passed).unwrap();
    passed_json["attempts"].as_array_mut().unwrap().push(last);
    let passed: Task = serde_json::from_value(passed_json).unwrap();
    let mut timed_out = full.attempts[2].clone();
    timed_out.failure.as_mut().unwrap().kind = sushiai_orch::task::FailureKind::TimeBudget;
    timed_out.failure.as_mut().unwrap().detail = "Attempt reached its time budget".into();
    timed_out.verify.clear();
    let pr_body = "## Summary\n\nAdds a Save button and tests for it.";

    vec![
        (
            "build_brief_bare",
            sushiai_orch::engine::attempts::brief::build_brief(&bare, "", ""),
        ),
        (
            "build_brief_full",
            sushiai_orch::engine::attempts::brief::build_brief(&full, status, stat),
        ),
        (
            "build_resume_brief",
            sushiai_orch::engine::attempts::resume::build_resume_brief(
                &full,
                &full.attempts[2],
                "## Messages\n\nA peer says hello.",
                Some(advice),
            ),
        ),
        (
            "build_resume_brief_time_budget",
            sushiai_orch::engine::attempts::resume::build_resume_brief(&full, &timed_out, "", None),
        ),
        (
            "build_pick_brief",
            sushiai_orch::engine::attempts::pick::build_pick_brief(
                &full,
                &PickCandidate {
                    verify: &pick_a,
                    diff,
                },
                &PickCandidate {
                    verify: &pick_b,
                    diff: "+other",
                },
            ),
        ),
        (
            "build_advisor_brief",
            sushiai_orch::engine::attempts::advisor::build_advisor_brief(
                &full,
                diff,
                "`npm run lint` exited 1.",
            ),
        ),
        (
            "advisor_block",
            sushiai_orch::engine::attempts::advisor::advisor_block(advice),
        ),
        (
            "build_pr_draft_brief",
            sushiai_orch::delivery::brief::build_pr_draft_brief(&full, diff),
        ),
        (
            "build_pr_truth_brief",
            sushiai_orch::delivery::brief::build_pr_truth_brief(&passed, pr_body, diff),
        ),
        (
            "build_pr_truth_brief_no_results",
            sushiai_orch::delivery::brief::build_pr_truth_brief(&bare, pr_body, diff),
        ),
        (
            "discarded_block",
            sushiai_orch::engine::attempts::context::discarded_block(&[
                "tests/a.test.ts".to_string(),
                "tests/b.test.ts".to_string(),
            ]),
        ),
        (
            "coordination_block",
            sushiai_orch::engine::attempts::context::coordination_block(
                "t-fixture",
                &[
                    (
                        "t-other".to_string(),
                        message("m1", "question", "Which file?"),
                    ),
                    (
                        "t-third".to_string(),
                        message("m2", "reply", "settings.json"),
                    ),
                    ("t-lead".to_string(), message("m3", "message", "FYI")),
                ],
            ),
        ),
        (
            "landed_dependencies_block",
            sushiai_orch::engine::attempts::dependencies::landed_dependencies_block(
                &with_deps, &all, None,
            ),
        ),
        (
            "conflict_block",
            sushiai_orch::engine::attempts::context::conflict_block(
                "criterion [0] wants it hidden, criterion [1] clicked",
            ),
        ),
        (
            "conflict_only_block_files",
            sushiai_orch::engine::attempts::context::conflict_only_block(&[
                "src/a.ts".to_string(),
                "src/b.ts".to_string(),
            ]),
        ),
        (
            "conflict_only_block_bare",
            sushiai_orch::engine::attempts::context::conflict_only_block(&[]),
        ),
        (
            "parent_failure_block",
            sushiai_orch::engine::attempts::context::parent_failure_block("`npm run ci` exited 1."),
        ),
        (
            "review_conflict_block",
            sushiai_orch::verify::review::brief::review_conflict_block(
                "criterion [0] wants it hidden, criterion [1] clicked",
            ),
        ),
        ("where_to_look_block", where_to_look()),
        (
            "relay_block",
            sushiai_orch::engine::attempts::dependencies::relay_block(&relayed, &all),
        ),
        (
            "build_plan_brief",
            sushiai_orch::engine::planning::prompt::build_plan_brief(
                REQUEST,
                &Variant::default(),
                PAST_WORK,
            ),
        ),
        (
            "build_plan_brief_batch",
            sushiai_orch::engine::planning::prompt::build_plan_brief(REQUEST, &batch(), PAST_WORK),
        ),
        (
            "build_plan_brief_no_past_work",
            sushiai_orch::engine::planning::prompt::build_plan_brief(
                REQUEST,
                &Variant::default(),
                "",
            ),
        ),
        (
            "build_subtask_plan_brief",
            sushiai_orch::engine::planning::prompt::build_subtask_plan_brief(
                REQUEST,
                &Variant::default(),
                PAST_WORK,
            ),
        ),
        (
            "build_subtask_plan_brief_batch",
            sushiai_orch::engine::planning::prompt::build_subtask_plan_brief(
                REQUEST,
                &batch(),
                PAST_WORK,
            ),
        ),
        (
            "build_plan_correction_brief",
            sushiai_orch::engine::planning::prompt::build_plan_correction_brief(
                " criteria 0 and 1 clash \n",
                &batch(),
                true,
            ),
        ),
        (
            "build_plan_correction_brief_subtask",
            sushiai_orch::engine::planning::prompt::build_plan_correction_brief(
                "criteria 0 and 1 clash",
                &Variant::default(),
                false,
            ),
        ),
        (
            "build_plan_continue_brief",
            sushiai_orch::engine::planning::prompt::build_plan_continue_brief(&batch(), true),
        ),
        (
            "build_plan_continue_brief_subtask",
            sushiai_orch::engine::planning::prompt::build_plan_continue_brief(
                &Variant::default(),
                false,
            ),
        ),
        (
            "build_plan_retry_brief",
            sushiai_orch::engine::planning::prompt::build_plan_retry_brief(
                REQUEST,
                &batch(),
                PAST_WORK,
            ),
        ),
        (
            "build_subtask_plan_retry_brief",
            sushiai_orch::engine::planning::prompt::build_subtask_plan_retry_brief(
                REQUEST,
                &Variant::default(),
                PAST_WORK,
            ),
        ),
        (
            "scoped_checks_block",
            sushiai_orch::engine::planning::prompt::scoped_checks_block(&scoped, "/repo"),
        ),
        (
            "build_audit_brief",
            sushiai_orch::engine::audit::brief::build_audit_brief(),
        ),
        (
            "build_proposal_brief",
            sushiai_orch::engine::evolution::proposal::build_proposal_brief(&proposal),
        ),
        (
            "build_proposal_brief_no_excerpts",
            sushiai_orch::engine::evolution::proposal::build_proposal_brief(&proposal_empty),
        ),
        (
            "build_review_brief",
            sushiai_orch::verify::review::brief::build_review_brief(
                &review,
                &sushiai_orch::verify::review::brief::ReviewEvidence {
                    implementer_note: note,
                    agent_decisions: &agent_decisions,
                    verify_results: &review_verify(),
                    screenshots: &screenshots,
                    evidence_from: Some(3),
                    declines: &declines,
                },
                diff,
                &some_lenses,
            ),
        ),
        (
            "build_review_brief_first",
            sushiai_orch::verify::review::brief::build_review_brief(
                &first_review,
                &sushiai_orch::verify::review::brief::ReviewEvidence {
                    implementer_note: note,
                    agent_decisions: &[],
                    verify_results: &[],
                    screenshots: &[],
                    evidence_from: None,
                    declines: &[],
                },
                diff,
                &Lenses::ALL,
            ),
        ),
        (
            "build_narrow_review_brief",
            sushiai_orch::verify::review::brief::build_narrow_review_brief(
                &review,
                &sushiai_orch::verify::review::brief::ReviewEvidence {
                    implementer_note: note,
                    agent_decisions: &agent_decisions,
                    verify_results: &review_verify(),
                    screenshots: &screenshots,
                    evidence_from: None,
                    declines: &declines,
                },
                &narrow,
            ),
        ),
        (
            "build_narrow_review_brief_settled",
            sushiai_orch::verify::review::brief::build_narrow_review_brief(
                &first_review,
                &sushiai_orch::verify::review::brief::ReviewEvidence {
                    implementer_note: note,
                    agent_decisions: &[],
                    verify_results: &[],
                    screenshots: &[],
                    evidence_from: None,
                    declines: &[],
                },
                &narrow_settled,
            ),
        ),
        (
            "review_retry_note",
            sushiai_orch::verify::review::brief::review_retry_note(),
        ),
        (
            "review_lens_block_all",
            sushiai_orch::verify::review::routing::review_lens_block(&Lenses::ALL),
        ),
        (
            "review_lens_block_some",
            sushiai_orch::verify::review::routing::review_lens_block(&some_lenses),
        ),
        (
            "review_lens_block_none",
            sushiai_orch::verify::review::routing::review_lens_block(&no_lenses),
        ),
        (
            "build_route_brief",
            sushiai_orch::verify::review::routing::build_route_brief(&full, &changed),
        ),
        (
            "build_route_brief_bare",
            sushiai_orch::verify::review::routing::build_route_brief(&bare, &[]),
        ),
    ]
}

/// The first byte where `a` and `b` differ, with a little context.
fn first_difference(golden: &str, rendered: &str) -> String {
    let at = golden
        .bytes()
        .zip(rendered.bytes())
        .position(|(g, r)| g != r)
        .unwrap_or(golden.len().min(rendered.len()));
    let around = |s: &str| {
        let from = at.saturating_sub(40);
        String::from_utf8_lossy(&s.as_bytes()[from.min(s.len())..(at + 40).min(s.len())])
            .into_owned()
    };
    format!(
        "differs at byte {at} (golden {} bytes, rendered {} bytes)\n  golden:   {:?}\n  rendered: {:?}",
        golden.len(),
        rendered.len(),
        around(golden),
        around(rendered)
    )
}

#[test]
fn every_brief_matches_its_golden() {
    let dir = golden_dir();
    let update = std::env::var("UPDATE_PROMPT_SNAPSHOTS").as_deref() == Ok("1");
    let renders = renders();
    let mut failures = Vec::new();
    for (kind, rendered) in &renders {
        assert!(!rendered.is_empty(), "{kind} rendered nothing");
        let path = dir.join(format!("{kind}.txt"));
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, rendered).unwrap();
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(golden) if &golden == rendered => {}
            Ok(golden) => failures.push(format!("{kind}: {}", first_difference(&golden, rendered))),
            Err(e) => failures.push(format!("{kind}: no golden at {} ({e})", path.display())),
        }
    }
    assert!(
        failures.is_empty(),
        "{} brief(s) differ from their goldens (UPDATE_PROMPT_SNAPSHOTS=1 rewrites them):\n{}",
        failures.len(),
        failures.join("\n")
    );

    // A golden no kind renders any more is stale.
    let stale: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".txt"))
        .filter(|name| {
            !renders
                .iter()
                .any(|(kind, _)| *name == format!("{kind}.txt"))
        })
        .collect();
    assert!(stale.is_empty(), "goldens without a brief kind: {stale:?}");
}
