use super::*;
use crate::engine::plan_gate::publication::{GateInput, GateProposal};
use crate::engine::plan_gate::GateAction;
use crate::engine::test_support::{task_with_status, test_app};
use crate::task::TaskStatus;

#[test]
fn a_completed_review_charges_its_helper_once_even_when_the_owner_stopped_or_changed() {
    for stopped in [true, false] {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Drafting);
        task.fixture_data().criteria = vec!["Keep acceptance".into()];
        app.store.save_task(&task).unwrap();
        let input = GateInput::capture(&task);
        app.update_task(&task.id, |fresh| {
            if stopped {
                fresh.resume_phase(TaskStatus::Stopped, 2);
            } else {
                fresh.fixture_data().criteria = vec!["Owner's new acceptance".into()];
            }
        })
        .unwrap();
        let selected = SelectedReview {
            route: app.settings.read().unwrap().routes[0].clone(),
            source: "planReview".into(),
        };
        let review = selected.complete(&app, &task, JudgeRun {
            reply: Ok("{\"findings\":[{\"kind\":\"wrong-approach\",\"severity\":\"critical\",\"text\":\"Change it\"}]}".into()),
            cost: 0.4,
            timeout_note: None,
        }).unwrap_or_else(|_| panic!("the completed helper receipt must be charged"));
        assert_eq!(
            app.store.load_task(&task.id).unwrap().unwrap().cost_usd,
            0.4
        );
        let action = GateProposal { input, review }
            .publish(&app, &task.id, &CancelToken::new())
            .unwrap();
        assert!(matches!(
            (stopped, action),
            (true, GateAction::Cancelled) | (false, GateAction::Changed)
        ));
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.cost_usd, 0.4);
        assert!(!saved.plan_gate.reviewed && !saved.plan_gate.replanned);
        assert!(saved.plan_gate.findings.is_empty());
        assert!(saved.decisions.is_empty());
        if stopped {
            assert_eq!(saved.status, TaskStatus::Stopped);
        } else {
            assert_eq!(saved.criteria, ["Owner's new acceptance"]);
        }
    }
}
