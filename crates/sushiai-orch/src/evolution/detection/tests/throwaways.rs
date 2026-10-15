use super::*;

#[test]
fn evolution_throwaway_script_written_then_removed() {
    let mut t = task("t", 10);
    t.fixture_data().attempts = vec![attempt_with_failure(1, "s")];
    let evs = vec![
        call(
            "Write",
            json!({"file_path": "/wt/scratch/probe.py", "content": "x"}),
        ),
        call("Bash", json!({"command": "python /wt/scratch/probe.py"})),
        read("/other"),
        call("Bash", json!({"command": "rm -f probe.py"})),
    ];
    let s = run(&t, &[], &[("t", 1, evs)]);
    let x = of_kind(&s, SignalKind::ThrowawayScript);
    assert_eq!(x.len(), 1);
    assert_eq!(x[0].wasted_calls, 3);
    assert_eq!(
        (x[0].excerpt_ref.from_line, x[0].excerpt_ref.to_line),
        (1, 4)
    );
    // written and kept: nothing
    let kept = vec![call("Write", json!({"file_path": "/wt/a.rs"}))];
    assert!(of_kind(
        &run(&t, &[], &[("t", 1, kept)]),
        SignalKind::ThrowawayScript
    )
    .is_empty());
}

#[test]
fn evolution_throwaway_only_counts_rm_operands_of_files_the_attempt_dropped() {
    let write = |p: &str| call("Write", json!({"file_path": p, "content": "x"}));
    let bash = |c: &str| call("Bash", json!({"command": c}));
    let cases: Vec<(&str, Vec<Value>, Vec<&str>, usize)> = vec![
        // Real throwaway.
        (
            "rm the file",
            vec![write("/wt/artifacts/.driver.mjs"), bash("rm -f artifacts/.driver.mjs")],
            vec![],
            1,
        ),
        // `rm` of something else while the command line names the file.
        (
            "rm of another file",
            vec![
                write("/wt/tests/answer_policy.rs"),
                bash("rm -f /tmp/judge-brief.txt; cargo test --test answer_policy tests/answer_policy.rs"),
            ],
            vec![],
            0,
        ),
        // A sibling with a longer name is not the file.
        (
            "rm of a longer name",
            vec![
                write("/wt/orchd/src/chat_tools.rs"),
                bash("rm orchd/src/chat_tools.rs.add"),
            ],
            vec![],
            0,
        ),
        // The attempt kept the file in its diff.
        (
            "kept source",
            vec![write("/wt/tests/past_work.rs"), bash("rm tests/past_work.rs")],
            vec!["tests/past_work.rs"],
            0,
        ),
        // `rm` inside a heredoc body is text, not a command.
        (
            "heredoc text",
            vec![
                write("/wt/scratch.py"),
                bash("cat <<'EOF' > notes.txt\nrm scratch.py\nEOF"),
            ],
            vec![],
            0,
        ),
    ];
    for (name, evs, changed, want) in cases {
        let mut t = task("t", 10);
        let mut a = attempt_with_failure(1, "s");
        a.changed_files = changed.iter().map(|c| c.to_string()).collect();
        t.fixture_data().attempts = vec![a];
        let s = run(&t, &[], &[("t", 1, evs)]);
        assert_eq!(
            of_kind(&s, SignalKind::ThrowawayScript).len(),
            want,
            "{name}"
        );
    }
}
