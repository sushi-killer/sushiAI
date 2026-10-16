use crate::engine::permissions::denials::scratch_lines;
use crate::engine::permissions::denials::unhandled_denials;
use crate::engine::permissions::gate::web_allowed;
use crate::harness;
use crate::policy::permissions::Subject;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::Path;

fn denial(tool: &str, input: serde_json::Value) -> harness::PermissionDenial {
    harness::PermissionDenial {
        tool: tool.to_string(),
        input,
    }
}

#[test]
fn only_refusals_orchd_did_not_decide_are_asked_about() {
    let outcome = harness::RunOutcome {
        permission_denials: vec![
            denial("mcp__slack__post", json!({})),
            denial("mcp__slack__post", json!({"again": 1})),
            denial("Write", json!({"file_path": "/wt/.claude/a.md"})),
            denial("Write", json!({"file_path": "/wt/.claude/done.md"})),
            denial("Bash", json!({"command": "git push"})),
            denial("mcp__sushiai-messages__peer_send", json!({})),
            denial("Task", json!({})),
        ],
        ..Default::default()
    };
    let handled: BTreeSet<String> = [".claude/done.md".to_string()].into();
    let subjects = unhandled_denials(&handled, Path::new("/wt"), &outcome, &[]);
    assert_eq!(
        subjects,
        vec![
            Subject::Tool("mcp__slack__post".into()),
            Subject::Path(".claude/a.md".into())
        ]
    );
}

#[test]
fn web_calls_are_decided_by_the_allowed_domains() {
    let fetch = |url: &str| json!({"url": url});
    let open = vec!["*".to_string()];
    let some = vec!["docs.rs".to_string(), "*.example.org".to_string()];
    assert!(web_allowed(&open, "WebSearch", None));
    assert!(web_allowed(
        &some,
        "WebFetch",
        Some(&fetch("https://docs.rs/serde/latest"))
    ));
    assert!(web_allowed(
        &some,
        "WebFetch",
        Some(&fetch("https://api.Example.org:8443/x?y#z"))
    ));
    assert!(!web_allowed(
        &some,
        "WebFetch",
        Some(&fetch("https://evil.test/docs.rs"))
    ));
    assert!(!web_allowed(
        &some,
        "WebFetch",
        Some(&fetch("https://notdocs.rs/"))
    ));
    assert!(!web_allowed(
        &some,
        "WebFetch",
        Some(&fetch("https://u@evil.test/"))
    ));
    for url in [
        "https://evil.test\\@docs.rs/",
        "https://evil.test\\.docs.rs/",
        "not a url",
        "ftp://docs.rs/x",
    ] {
        assert!(!web_allowed(&some, "WebFetch", Some(&fetch(url))), "{url}");
    }
    assert!(web_allowed(
        &open,
        "WebFetch",
        Some(&fetch("https://anything.example/x"))
    ));
    assert!(!web_allowed(&some, "WebSearch", None));
    assert!(!web_allowed(
        &[],
        "WebFetch",
        Some(&fetch("https://docs.rs/"))
    ));
}

#[test]
fn a_refused_write_to_a_temp_dir_is_recorded_and_raises_no_question() {
    let outcome = harness::RunOutcome {
        permission_denials: vec![
            denial("Write", json!({"file_path": "/tmp/scratch/notes.txt"})),
            denial("Write", json!({"file_path": "/tmp/scratch/notes.txt"})),
            denial("Write", json!({"file_path": "/etc/hosts"})),
            denial("Write", json!({"file_path": "/wt/docs/b.md"})),
            denial("Bash", json!({"file_path": "/tmp/x"})),
        ],
        ..Default::default()
    };
    let none = BTreeSet::new();
    assert_eq!(
        unhandled_denials(&none, Path::new("/wt"), &outcome, &[]),
        vec![
            Subject::Path("/etc/hosts".into()),
            Subject::Path("docs/b.md".into())
        ]
    );
    let lines = scratch_lines(Path::new("/wt"), &outcome);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("/tmp/scratch/notes.txt"));
}

#[test]
fn a_refused_write_to_a_protected_path_raises_no_question() {
    let outcome = harness::RunOutcome {
        permission_denials: vec![
            denial("Write", json!({"file_path": "/wt/tests/a.rs"})),
            denial("Edit", json!({"file_path": "/wt/docs/b.md"})),
        ],
        ..Default::default()
    };
    let none = BTreeSet::new();
    let protected = vec!["tests/a.rs".to_string()];
    assert_eq!(
        unhandled_denials(&none, Path::new("/wt"), &outcome, &protected),
        vec![Subject::Path("docs/b.md".into())]
    );
    assert_eq!(
        unhandled_denials(&none, Path::new("/wt"), &outcome, &[]).len(),
        2
    );
}
