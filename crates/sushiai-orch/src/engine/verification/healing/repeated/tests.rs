use super::*;
use crate::engine::persistence::contract::tests::{assert_owner_applies, queue_owner};
use crate::engine::questions::parking::WaitEnd;
use crate::engine::test_support::{task_with_status, test_app};
use crate::task::TaskStatus;

fn proposal() -> CriterionProposal {
    CriterionProposal {
        old: "Original criterion".into(),
        repeated: Repeated {
            criterion: 0,
            earlier: (1, "first".into()),
            current: (2, "again".into()),
        },
        ruling: Some(Ruling::Infeasible {
            criterion: "Judge criterion".into(),
            reason: "infeasible".into(),
        }),
    }
}

#[test]
fn an_accepted_owner_edit_defers_the_criterion_ruling_and_its_assumption() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().criteria = vec!["Original criterion".into()];
    app.store.save_task(&task).unwrap();
    let input = ContractInput::capture(&task);
    queue_owner(&app, &task);
    assert!(matches!(
        proposal().publish(&app, &mut task, &input, &CancelToken::new()),
        Ok(Publication::Deferred(WaitEnd::Changed))
    ));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.criteria, ["Original criterion"]);
    assert!(saved.assumptions.is_empty());
    assert!(saved.brief_check.judged.is_empty());
    assert_owner_applies(&app, &task);
}

#[test]
fn an_admitted_criterion_ruling_updates_fresh_and_local_contracts() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().criteria = vec!["Original criterion".into()];
    app.store.save_task(&task).unwrap();
    let input = ContractInput::capture(&task);
    assert!(matches!(
        proposal().publish(&app, &mut task, &input, &CancelToken::new()),
        Ok(Publication::Applied(true))
    ));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.criteria, ["Judge criterion"]);
    assert_eq!(saved.criteria, task.criteria);
    assert_eq!(saved.assumptions.len(), 1);
    assert_eq!(task.assumptions.len(), 1);
    assert_eq!(saved.decisions, task.decisions);
}
