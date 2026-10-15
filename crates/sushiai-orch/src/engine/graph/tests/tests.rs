#[cfg(test)]
use crate::engine::test_support::init_git_repo;
#[cfg(test)]
use crate::engine::test_support::test_app;
use crate::engine::verification::runner::run_checks;
use crate::harness::cancel::CancelToken;

#[tokio::test]
async fn a_pass_under_one_base_is_not_reused_under_another() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let counter = dir.path().join("counter");
    let commands = vec![format!(
        "echo \"$ORCHD_BASE_SHA\" >> '{}'",
        counter.display()
    )];
    let cancel = CancelToken::new();
    let run = |base| {
        run_checks(
            crate::engine::verification::runner::CheckRunner::new(&app, &repo, None, base, &cancel),
            &commands,
        )
    };
    let (outcomes, lines) = run("base-a").await;
    assert_eq!(outcomes[0].code, Some(0));
    assert!(lines.is_empty());
    let (outcomes, lines) = run("base-a").await;
    assert_eq!(outcomes[0].ms, 0);
    assert_eq!(lines.len(), 1);
    let (outcomes, lines) = run("base-b").await;
    assert_eq!(outcomes[0].code, Some(0));
    assert!(lines.is_empty());
    assert_eq!(
        std::fs::read_to_string(counter).unwrap(),
        "base-a\nbase-b\n"
    );
}

#[tokio::test]
async fn a_timed_out_final_is_never_recorded_as_a_pass() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let commands = vec!["exit 137".into(), "false".into(), "true".into()];
    let cancel = CancelToken::new();
    let run = || {
        run_checks(
            crate::engine::verification::runner::CheckRunner::new(
                &app, &repo, None, "base", &cancel,
            ),
            &commands,
        )
    };
    let (first, lines) = run().await;
    assert_eq!(first[0].code, Some(137));
    assert_eq!(first[1].code, Some(1));
    assert_eq!(first[2].code, Some(0));
    assert_eq!(lines.len(), 1);
    let (second, lines) = run().await;
    assert_eq!(second[0].code, Some(137));
    assert_eq!(second[1].code, Some(1));
    assert_eq!(second[2].ms, 0);
    assert_eq!(lines.len(), 2);
}
