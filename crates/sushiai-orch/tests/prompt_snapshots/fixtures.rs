use serde_json::{json, Value};
use sushiai_orch::policy::variant::Variant;
use sushiai_orch::task::messages::Message;
use sushiai_orch::task::{Task, VerifyOutcome};

/// `fields` over a minimal task.
pub(super) fn task(fields: Value) -> Task {
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

pub(super) fn verify(command: &str, code: Option<i32>, tail: &str) -> Value {
    json!({ "command": command, "code": code, "tail": tail, "ms": 1200 })
}

pub(super) fn attempt(n: u32, stage: &str, status: &str, fields: Value) -> Value {
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
pub(super) fn attempts() -> Value {
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
pub(super) fn full_task() -> Task {
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
pub(super) fn dependency(id: &str) -> Task {
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

pub(super) fn message(id: &str, kind: &str, text: &str) -> Message {
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

pub(super) fn outcome(command: &str, code: Option<i32>, tail: &str) -> VerifyOutcome {
    serde_json::from_value(verify(command, code, tail)).expect("fixture verify deserializes")
}

pub(super) fn batch() -> Variant {
    Variant {
        batch_questions: true,
        ..Variant::default()
    }
}

pub(super) const REQUEST: &str =
    "  Add a Save button to the settings dialog.\n\n1. It writes settings.json.\n2. It shows an error when the write fails.\n";
pub(super) const PAST_WORK: &str = "## Past work\n\n- t-old: added the settings dialog\n\n";

pub(super) fn review_verify() -> Vec<VerifyOutcome> {
    vec![
        outcome("npm test -- settings", Some(0), "  12 passing\n"),
        outcome("npm run lint", Some(1), "src/settings.ts:12 no-unused-vars"),
        outcome("held-out check (criterion 1)", Some(0), "hidden"),
    ]
}

/// A review of the latest attempt: the one before it reported findings.
pub(super) fn review_task() -> Task {
    let mut t = serde_json::to_value(full_task()).unwrap();
    let mut a = t["attempts"][3].clone();
    a["n"] = json!(5);
    a["status"] = json!("running");
    a["review"] = Value::Null;
    a["failure"] = Value::Null;
    t["attempts"].as_array_mut().unwrap().push(a);
    serde_json::from_value(t).unwrap()
}

pub(super) fn where_to_look() -> String {
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
