use super::record_publication;
use crate::delivery::pr::publication::{PrContent, Publication, PublishedPr};
use crate::delivery::pr::request::PrParams;
use crate::engine::test_support::{task_with_status, test_app};
use crate::task::questions::{WAIVED, WAIVER_BY};
use crate::task::{Assumption, QuestionKind, TaskStatus};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::Duration;

#[test]
fn a_pr_receipt_waits_for_owner_publication_and_preserves_the_stop_and_revocation() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Done);
    task.record_assumption(Assumption {
        question: "Accept the inherited failure?".into(),
        answer: WAIVED.into(),
        evidence: "The same named failure was observed on the base".into(),
        by: WAIVER_BY.into(),
        kind: Some(QuestionKind::PreexistingFailure),
        attempt: None,
        commands: vec!["check".into()],
        overturned: false,
        owner_answer: None,
    });
    app.store.save_task(&task).unwrap();
    let lock = app.task_lock(&task.id);
    let guard = lock.lock().unwrap();
    let mut owner = app.store.load_task(&task.id).unwrap().unwrap();
    let (started, entered) = channel();
    let (finished, completion) = channel();
    let worker_app = app.clone();
    let id = task.id.clone();
    let worker = std::thread::spawn(move || {
        let request: PrParams = serde_json::from_value(serde_json::json!({"id":id})).unwrap();
        let publication = Publication {
            content: PrContent {
                branch: "task/receipt".into(),
                title: "Preserve owner state".into(),
                body: "Synthetic publication receipt".into(),
            },
            repo: "/synthetic-repo".into(),
            base: "work".into(),
            local: "refs/heads/task/receipt".into(),
        };
        let receipt = PublishedPr {
            url: "https://github.example/acme/app/pull/7".into(),
            verb: "opened",
            head: "1234567890abcdef".into(),
        };
        started.send(()).unwrap();
        finished
            .send(record_publication(
                &worker_app,
                &request,
                &publication,
                receipt,
            ))
            .unwrap();
    });
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let early = completion.recv_timeout(Duration::from_millis(250));
    let serialized = matches!(&early, Err(RecvTimeoutError::Timeout));
    owner.overturn_assumption(0, "This check must pass".into());
    owner.interrupt(None, true, "Owner: stopped".into(), 10);
    app.store.save_task(&owner).unwrap();
    drop(guard);
    let result = match early {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => completion.recv_timeout(Duration::from_secs(5)).unwrap(),
        Err(error) => panic!("publication thread exited before recording the receipt: {error}"),
    }
    .unwrap();
    worker.join().unwrap();
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(
        saved.pr_url.as_deref(),
        Some("https://github.example/acme/app/pull/7"),
        "the in-flight Owner save lost the factual PR receipt"
    );
    assert!(serialized, "PR receipt bypassed the current Owner mutation");
    assert_eq!(result["status"], "stopped");
    assert_eq!(saved.status, TaskStatus::Stopped);
    assert!(saved.queue.stopped_explicitly);
    assert!(saved.assumptions[0].overturned);
    assert_eq!(
        saved.assumptions[0].owner_answer.as_deref(),
        Some("This check must pass")
    );
    assert_eq!(saved.pr_branch.as_deref(), Some("task/receipt"));
    assert!(saved
        .decisions
        .iter()
        .any(|line| line.contains("head 1234567")));
}
