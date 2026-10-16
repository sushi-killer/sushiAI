use super::*;
use crate::task::messages::MessageKind;
use crate::task::{AskedBy, Question, QuestionKind};

#[test]
fn evolution_graph_conflict_failed_parent_and_sibling_question() {
    let mut parent = task("p", 1);
    parent.mark_failed();
    let mut a = task("a", 2);
    a.fixture_data().parent = Some("p".into());
    a.fixture_data().title = "Add login".into();
    let mut b = task("b", 3);
    b.fixture_data().parent = Some("p".into());
    b.fixture_data().title = "Wire billing".into();
    let mut conflict = attempt_with_failure(1, "s");
    conflict.failure = Some(failure(
        FailureKind::Verify,
        "main moved. These files conflict: x.rs. Resolve",
        "s",
    ));
    b.fixture_data().attempts = vec![conflict];
    b.fixture_data().question = Some(Question::new(
        "Should I reuse what Add login built?",
        vec![],
        QuestionKind::AgentQuestion,
        AskedBy::Implement,
        crate::clock::now_ms(),
    ));
    let repo = vec![parent.clone(), a.clone(), b.clone()];
    let run_for = |t: &Task, msgs: &[Message]| {
        Detection::for_task(t, &AttemptEvents::new(), &Settings::default()).finish(&repo, msgs, &[])
    };
    // parent failed with all children done
    let mut done_a = a.clone();
    done_a.fixture_data().status = TaskStatus::Done;
    let mut done_b = b.clone();
    done_b.fixture_data().status = TaskStatus::Done;
    let s = Detection::for_task(&parent, &AttemptEvents::new(), &Settings::default()).finish(
        &[parent.clone(), done_a, done_b],
        &[],
        &[],
    );
    assert_eq!(of_kind(&s, SignalKind::Graph).len(), 1);
    // conflict + undeclared sibling in the question
    let s = run_for(&b, &[]);
    assert_eq!(of_kind(&s, SignalKind::Graph).len(), 2);
    // a declared dependency is not a graph problem
    let mut declared = b.clone();
    declared.fixture_data().depends_on = vec!["a".into()];
    declared.fixture_data().attempts.clear();
    assert!(of_kind(&run_for(&declared, &[]), SignalKind::Graph).is_empty());
    // a sent message naming the sibling's id counts
    let mut quiet = b.clone();
    quiet.fixture_data().attempts.clear();
    quiet.fixture_data().question = None;
    let msg = Message {
        id: "m".into(),
        repo: "/repo".into(),
        from: "b".into(),
        to: "a".into(),
        kind: MessageKind::Message,
        text: "hey, done?".into(),
        reply_to: None,
        ts: 0,
        delivered: false,
        delivered_at: None,
    };
    assert!(of_kind(
        &run_for(&quiet, std::slice::from_ref(&msg)),
        SignalKind::Graph
    )
    .is_empty());
    let msg = Message {
        text: "did Add login finish?".into(),
        ..msg
    };
    assert_eq!(
        of_kind(&run_for(&quiet, &[msg]), SignalKind::Graph).len(),
        1
    );
    // conflict outside a subtask is a moved base, not a sibling conflict
    let mut solo = b.clone();
    solo.fixture_data().parent = None;
    solo.fixture_data().question = None;
    assert!(of_kind(&run_for(&solo, &[]), SignalKind::Graph).is_empty());
}
