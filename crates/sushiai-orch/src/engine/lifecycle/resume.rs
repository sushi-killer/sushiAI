use crate::engine::decision::last_review_repeated;
use crate::policy::routing::Route;
use crate::task::Attempt;
use crate::task::FailureKind;
use crate::task::Stage;
use crate::task::Task;

pub(in crate::engine) fn resume_attempt<'a>(
    task: &'a Task,
    route: &Route,
    conflict_only: bool,
    best_of: bool,
) -> Option<&'a Attempt> {
    if conflict_only || best_of || last_review_repeated(task) {
        return None;
    }
    task.attempts
        .iter()
        .rfind(|a| a.stage == Stage::Implement)
        .filter(|a| {
            a.failure
                .as_ref()
                .is_some_and(|f| matches!(f.kind, FailureKind::Verify | FailureKind::TimeBudget))
                && a.session_id.as_ref().is_some_and(|id| !id.is_empty())
                && a.route_id == route.id
                && a.harness == route.harness
                && a.model == route.model.as_deref().unwrap_or_default()
        })
}
