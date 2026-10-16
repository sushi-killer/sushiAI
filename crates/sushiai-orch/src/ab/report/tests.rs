use super::*;
use crate::ab::{is_edit_event, work_breakdown, Breakdown};
use crate::policy::settings::WorkBuckets;
use crate::policy::variant::Variant;
use serde_json::Value;

fn report(tasks: &[Task]) -> String {
    report_with(tasks, None, &|_, _| None)
}

fn task(variant: Variant, status: TaskStatus, cost: f64, decisions: &[&str]) -> Task {
    let mut t: Task = serde_json::from_value(serde_json::json!({
        "id": "t", "title": "t", "goal": "g", "criteria": [], "verify": [],
        "repo": "/r", "worktree": "/w", "branch": "b", "baseSha": "s",
        "status": "queued", "tier": "standard", "createdAt": 0, "updatedAt": 0
    }))
    .unwrap();
    t.fixture_data().variant = Some(variant);
    t.fixture_data().status = status;
    t.fixture_data().cost_usd = cost;
    t.fixture_data().decisions = decisions.iter().map(|d| d.to_string()).collect();
    t
}

#[test]
fn median_of_an_even_count_is_the_mean_of_the_middle_two() {
    assert_eq!(median(vec![6.1, 0.66]), Some((6.1 + 0.66) / 2.0));
    assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
    assert_eq!(median(vec![]), None);
}

#[test]
fn report_puts_each_variant_on_its_own_row_with_its_own_numbers() {
    let fresh = Variant {
        advisor: true,
        ..Variant::default()
    };
    let tasks = vec![
        task(Variant::default(), TaskStatus::Done, 1.0, &["Owner: yes"]),
        task(Variant::default(), TaskStatus::Waiting, 3.0, &[]),
        task(fresh.clone(), TaskStatus::Done, 0.5, &[]),
    ];
    let mut old = task(Variant::default(), TaskStatus::Done, 9.0, &[]);
    old.fixture_data().variant = None;
    let tasks = [tasks, vec![old]].concat();
    let out = report(&tasks);
    assert!(
        out.contains("1 task(s) from before variants left out."),
        "{out}"
    );
    let rows: Vec<&str> = out.lines().skip(2).filter(|l| l.starts_with('|')).collect();
    assert_eq!(rows.len(), 2, "{out}");
    let resume = rows
        .iter()
        .find(|r| r.contains("\"advisor\":false"))
        .unwrap();
    assert!(resume.contains("| 2 | 1 |"), "{resume}");
    assert!(resume.contains("| 2.00 |"), "{resume}");
    assert!(
        resume.contains("| 0.5 |"),
        "owner answers per task: {resume}"
    );
    let fresh_row = rows
        .iter()
        .find(|r| r.contains("\"advisor\":true"))
        .unwrap();
    assert!(fresh_row.contains("| 1 | 1 |"), "{fresh_row}");
    assert!(fresh_row.contains("| 0.50 |"), "{fresh_row}");
    assert!(
        fresh_row.ends_with("| 0 | - | - |"),
        "nothing recorded: {fresh_row}"
    );
}

fn attempt(stage: &str, prefix: Option<u64>, cached: Option<u64>) -> Attempt {
    let mut a: Attempt = serde_json::from_value(serde_json::json!({
        "n": 1, "stage": stage, "routeId": "r", "harness": "claude", "model": "m",
        "reason": "x", "resumed": false, "startedAt": 0, "status": "passed"
    }))
    .unwrap();
    a.prefix_tokens = prefix;
    a.usage = cached.map(|c| crate::task::Usage {
        input: 0,
        output: 0,
        cached: c,
    });
    a
}

#[test]
fn report_shows_median_prefix_and_mean_cache_read_tokens_per_implement_attempt() {
    let variant = Variant::default();
    let mut a = task(variant.clone(), TaskStatus::Done, 1.0, &[]);
    a.fixture_data().attempts = vec![
        attempt("plan", Some(90_000), Some(90_000)),
        attempt("implement", Some(20_000), Some(1_000)),
        attempt("implement", None, Some(3_000)),
    ];
    let mut b = task(variant, TaskStatus::Done, 1.0, &[]);
    b.fixture_data().attempts = vec![
        attempt("implement", Some(30_000), Some(5_000)),
        attempt("implement", Some(22_000), None),
    ];
    let out = report(&[a, b]);
    let row = out.lines().nth(2).unwrap();
    // Median of 20k, 30k, 22k; mean cached of 1k, 3k, 5k.
    assert!(row.ends_with("| 22000 | 3000 |"), "{row}");
    assert!(out
        .lines()
        .next()
        .unwrap()
        .ends_with("| median prefix tokens | cache-read tokens/attempt |"));
}

#[test]
fn different_variants_each_get_their_own_row() {
    let evidence = Variant {
        review_evidence: true,
        ..Variant::default()
    };
    let mut a = task(evidence, TaskStatus::Done, 1.0, &[]);
    a.fixture_data().attempts = vec![attempt("implement", Some(15_000), Some(2_000))];
    let mut b = task(Variant::default(), TaskStatus::Done, 1.0, &[]);
    b.fixture_data().attempts = vec![attempt("implement", Some(30_000), Some(5_000))];

    let out = report(&[a, b]);
    let rows: Vec<&str> = out.lines().skip(2).filter(|l| l.starts_with('|')).collect();
    assert_eq!(rows.len(), 2, "{out}");
    let evidence_row = rows
        .iter()
        .find(|r| r.contains("\"reviewEvidence\":true"))
        .unwrap();
    assert!(evidence_row.ends_with("| 15000 | 2000 |"), "{evidence_row}");
    assert!(rows.iter().any(|r| !r.contains("\"reviewEvidence\":true")));
}

fn with_fingerprint(mut t: Task, model: &str, version: &str) -> Task {
    let mut a = attempt("implement", None, None);
    a.fingerprint = Some(crate::task::Fingerprint {
        models: vec![model.to_string()],
        harness: crate::policy::routing::Harness::Claude,
        harness_version: Some(version.to_string()),
        prompt_hash: "abcd1234".into(),
    });
    t.fixture_data().attempts = vec![a];
    t
}

#[test]
fn one_variant_on_two_real_models_gets_two_rows_and_a_mixed_line() {
    let v = Variant::default();
    let a = with_fingerprint(
        task(v.clone(), TaskStatus::Done, 1.0, &[]),
        "claude-sonnet-4-6",
        "2.1.200",
    );
    let b = with_fingerprint(
        task(v.clone(), TaskStatus::Done, 1.0, &[]),
        "claude-sonnet-5-5",
        "2.1.284",
    );
    let c = with_fingerprint(
        task(v, TaskStatus::Done, 1.0, &[]),
        "claude-sonnet-5-5",
        "2.1.284",
    );
    let out = report(&[a, b, c]);
    let rows: Vec<&str> = out.lines().skip(2).filter(|l| l.starts_with('|')).collect();
    assert_eq!(rows.len(), 2, "{out}");
    let old = rows
        .iter()
        .find(|r| r.contains("claude-sonnet-4-6 (2.1.200)"))
        .unwrap();
    assert!(old.contains("| 1 | 1 |"), "{old}");
    let new = rows
        .iter()
        .find(|r| r.contains("claude-sonnet-5-5 (2.1.284)"))
        .unwrap();
    assert!(new.contains("| 2 | 2 |"), "{new}");
    assert!(out.contains("mixes 2 fingerprints"), "{out}");
}

#[test]
fn tasks_without_fingerprints_report_with_a_dash_and_no_mixed_line() {
    let mut t = task(Variant::default(), TaskStatus::Done, 1.0, &[]);
    t.fixture_data().attempts = vec![attempt("implement", None, None)];
    assert!(t.attempts[0].fingerprint.is_none());
    let out = report(&[t]);
    let row = out.lines().nth(2).unwrap();
    assert!(row.contains("| - | 1 | 1 |"), "{row}");
    assert!(!out.contains("mixes"), "{out}");
}

fn ev(parent: Option<&str>, name: &str, input: Value) -> Value {
    serde_json::json!({
        "type": "assistant",
        "parent_tool_use_id": parent,
        "message": {"content": [
            {"type": "text", "text": "thinking"},
            {"type": "tool_use", "id": "x", "name": name, "input": input}
        ]}
    })
}

#[test]
fn edit_events_are_top_level_edit_calls_and_codex_file_changes() {
    for name in ["Edit", "Write", "MultiEdit", "NotebookEdit", "apply_patch"] {
        assert!(is_edit_event(&ev(None, name, Value::Null)), "{name}");
    }
    assert!(!is_edit_event(&ev(None, "Read", Value::Null)));
    assert!(!is_edit_event(&ev(Some("sub"), "Edit", Value::Null)));
    let codex = |ty: &str, item: &str| serde_json::json!({"type": ty, "item": {"type": item}});
    assert!(is_edit_event(&codex("item.completed", "file_change")));
    assert!(is_edit_event(&codex("item.started", "file_change")));
    assert!(!is_edit_event(&codex(
        "item.completed",
        "command_execution"
    )));
    assert!(!is_edit_event(&serde_json::json!({"type": "result"})));
}

#[test]
fn breakdown_files_one_call_of_each_kind_and_ignores_subagent_calls() {
    let changed = vec!["src/a.ts".to_string()];
    let events = vec![
        ev(
            None,
            "Edit",
            serde_json::json!({"file_path": "/w/docs/LESSONS.md"}),
        ),
        ev(
            None,
            "Bash",
            serde_json::json!({"command": "node shot.mjs artifacts/x.png"}),
        ),
        ev(
            None,
            "Bash",
            serde_json::json!({"command": "cargo clippy --all-targets"}),
        ),
        ev(
            None,
            "Edit",
            serde_json::json!({"file_path": "/w/src/a.ts"}),
        ),
        ev(
            None,
            "Read",
            serde_json::json!({"file_path": "/w/src/other.ts"}),
        ),
        ev(
            Some("toolu_1"),
            "Bash",
            serde_json::json!({"command": "cargo test"}),
        ),
        serde_json::json!({"type": "user", "message": {"content": []}}),
    ];
    let rules = WorkBuckets {
        process: vec!["docs/LESSONS.md".into()],
        evidence: vec![],
    };
    let b = work_breakdown(&events, &changed, &[], &rules);
    assert_eq!(
        b,
        Breakdown {
            process: 1,
            evidence: 1,
            verify: 1,
            task: 1,
            explore: 1
        }
    );
}

#[test]
fn breakdown_uses_generic_rules_repo_patterns_and_the_task_s_own_verify() {
    let changed = vec!["src/a.ts".to_string()];
    let events = vec![
        ev(
            None,
            "Bash",
            serde_json::json!({"command": "sed -n 1,20p src/a.ts"}),
        ),
        ev(
            None,
            "Grep",
            serde_json::json!({"pattern": "x", "path": "src/a.ts"}),
        ),
        ev(
            None,
            "Bash",
            serde_json::json!({"command": "node tools/shoot.mjs"}),
        ),
        ev(
            None,
            "Bash",
            serde_json::json!({"command": "./scripts/check.sh --fast"}),
        ),
        ev(None, "Bash", serde_json::json!({"command": "npm run ci"})),
        ev(
            None,
            "Write",
            serde_json::json!({"file_path": "CHANGELOG.md"}),
        ),
    ];
    // Without repo patterns only generic rules apply: the changelog and
    // the custom screenshot script are exploration.
    let none = work_breakdown(&events, &changed, &[], &WorkBuckets::default());
    assert_eq!(
        none,
        Breakdown {
            process: 0,
            evidence: 0,
            verify: 1,
            task: 2,
            explore: 3
        }
    );
    let rules = WorkBuckets {
        process: vec!["CHANGELOG.md".into()],
        evidence: vec!["tools/shoot.mjs".into()],
    };
    let verify = vec!["./scripts/check.sh".to_string()];
    let b = work_breakdown(&events, &changed, &verify, &rules);
    assert_eq!(
        b,
        Breakdown {
            process: 1,
            evidence: 1,
            verify: 2,
            task: 2,
            explore: 0
        }
    );
}

#[test]
fn eval_report_keeps_only_that_set_and_adds_a_row_per_task_and_variant() {
    let fresh = Variant {
        advisor: true,
        ..Variant::default()
    };
    let mk = |v: Variant, set: Option<&str>, name: &str, cost: f64| {
        let mut t = task(v, TaskStatus::Done, cost, &[]);
        t.fixture_data().eval_set = set.map(String::from);
        t.fixture_data().eval_name = Some(name.into());
        t.fixture_data().attempts = vec![attempt("implement", None, None)];
        t
    };
    let tasks = vec![
        mk(Variant::default(), Some("set-1"), "alpha", 1.0),
        mk(fresh.clone(), Some("set-1"), "alpha", 2.0),
        mk(Variant::default(), Some("set-1"), "beta", 3.0),
        mk(Variant::default(), Some("other"), "gamma", 9.0),
        mk(Variant::default(), None, "delta", 9.0),
    ];
    let out = report_with(&tasks, Some("set-1"), &|_, _| {
        Some(Breakdown {
            process: 1,
            evidence: 0,
            verify: 1,
            task: 2,
            explore: 0,
        })
    });
    assert!(!out.contains("gamma") && !out.contains("delta"), "{out}");
    let per_task: Vec<&str> = out
        .lines()
        .skip_while(|l| !l.starts_with("| task |"))
        .skip(2)
        .filter(|l| l.starts_with('|'))
        .collect();
    assert_eq!(per_task.len(), 3, "{out}");
    assert!(
        per_task[0].starts_with("| alpha | `{") && per_task[0].contains("| 1 | 1 | 1.00 | 1.0 |"),
        "{out}"
    );
    assert!(per_task
        .iter()
        .any(|r| r.starts_with("| beta |") && r.contains("| 3.00 |")));
    // Variant rows: 2 arms, shares 25/0/25/50/0 and 4 calls.
    let row = out.lines().nth(2).unwrap();
    assert!(row.contains("| 25 | 0 | 25 | 50 | 0 | 4 |"), "{row}");
    // Without --eval the per-task table is absent.
    assert!(!report(&tasks).contains("| task | variant |"));
}

fn graded(v: &Variant, name: &str, status: TaskStatus, check: Option<i32>, cost: f64) -> Task {
    let mut t = task(v.clone(), status, cost, &[]);
    t.fixture_data().eval_set = Some("set-1".into());
    t.fixture_data().eval_name = Some(name.into());
    t.fixture_data().attempts = vec![attempt("implement", None, None)];
    if let Some(code) = check {
        t.fixture_data().eval_check_cmd = Some("true".into());
        t.fixture_data().eval_check = Some(crate::task::EvalCheck {
            code: Some(code),
            tail: String::new(),
        });
    }
    t
}

#[test]
fn eval_headline_gives_pass_k_and_cost_per_successful_task_per_variant() {
    use TaskStatus::{Done, Failed};
    let a = Variant::default();
    let b = Variant {
        advisor: true,
        ..Variant::default()
    };
    let mut tasks = vec![];
    // A: alpha 3/3 with no check; beta: one passing check, one failing
    // check on a done task, one failed task. $9 for 4 successes.
    for _ in 0..3 {
        tasks.push(graded(&a, "alpha", Done, None, 1.0));
    }
    tasks.push(graded(&a, "beta", Done, Some(0), 2.0));
    tasks.push(graded(&a, "beta", Done, Some(1), 2.0));
    tasks.push(graded(&a, "beta", Failed, None, 2.0));
    // B: alpha 2/3, beta 3/3. $4.50 for 5 successes.
    tasks.push(graded(&b, "alpha", Done, Some(0), 1.0));
    tasks.push(graded(&b, "alpha", Done, Some(0), 1.0));
    tasks.push(graded(&b, "alpha", Failed, None, 1.0));
    for _ in 0..3 {
        tasks.push(graded(&b, "beta", Done, Some(0), 0.5));
    }
    let out = report_with(&tasks, Some("set-1"), &|_, _| None);
    let row = |needle: &str| {
        out.lines()
            .find(|l| l.contains(needle) && l.contains("% (k=3)"))
            .unwrap_or_else(|| panic!("no headline row for {needle}: {out}"))
            .to_string()
    };
    let ra = row("\"advisor\":false");
    assert!(
        ra.contains("| 6 | 4/6 = 67% | 1/2 = 50% (k=3) | 2.25 |"),
        "{ra}"
    );
    let rb = row("\"advisor\":true");
    assert!(
        rb.contains("| 6 | 5/6 = 83% | 1/2 = 50% (k=3) | 0.90 |"),
        "{rb}"
    );
    let pair = |name: &str| {
        out.lines()
            .find(|l| l.starts_with(&format!("| {name} | ")) && l.contains(", $"))
            .unwrap_or_else(|| panic!("no paired row for {name}: {out}"))
            .to_string()
    };
    let alpha = pair("alpha");
    assert!(
        alpha.contains("3/3, $3.00") && alpha.contains("2/3, $3.00"),
        "{alpha}"
    );
    let beta = pair("beta");
    assert!(
        beta.contains("1/3, $6.00") && beta.contains("3/3, $1.50"),
        "{beta}"
    );
    assert!(!out.contains("baseline stale"), "{out}");
}

#[test]
fn a_done_task_whose_check_never_ran_is_not_a_success() {
    let mut t = graded(&Variant::default(), "alpha", TaskStatus::Done, None, 1.0);
    assert!(succeeded(&t));
    t.fixture_data().eval_check_cmd = Some("true".into());
    assert!(!succeeded(&t));
}

#[test]
fn a_variant_run_on_a_newer_fingerprint_marks_the_older_one_stale() {
    let v = Variant::default();
    let run = |model: &str, created: i64| {
        let mut t = graded(&v, "alpha", TaskStatus::Done, None, 1.0);
        t.fixture_data().created_at = created;
        t.fixture_data().attempts[0].fingerprint = Some(crate::task::Fingerprint {
            models: vec![model.into()],
            harness: crate::policy::routing::Harness::Claude,
            harness_version: Some("2.0".into()),
            prompt_hash: String::new(),
        });
        t
    };
    let tasks = vec![run("old-model", 1), run("new-model", 2)];
    let out = report_with(&tasks, Some("set-1"), &|_, _| None);
    assert!(
        out.contains("baseline stale: claude: old-model (2.0) -> claude: new-model (2.0)"),
        "{out}"
    );
    assert_eq!(out.matches("baseline stale").count(), 1, "{out}");
    assert!(!report_with(&tasks, None, &|_, _| None).contains("baseline stale"));
}

#[test]
fn tasks_without_eval_fields_still_load() {
    let t = task(Variant::default(), TaskStatus::Done, 1.0, &[]);
    assert!(t.eval_set.is_none() && t.eval_name.is_none());
    let json = serde_json::to_value(&t).unwrap();
    assert!(json.get("evalSet").is_none());
}
