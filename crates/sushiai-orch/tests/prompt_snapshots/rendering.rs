use super::fixtures;

#[path = "context.rs"]
mod context;
#[path = "delivery.rs"]
mod delivery;
#[path = "dependencies.rs"]
mod dependencies;
#[path = "first_review.rs"]
mod first_review;
#[path = "implementation.rs"]
mod implementation;
#[path = "narrow_review.rs"]
mod narrow_review;
#[path = "planning.rs"]
mod planning;
#[path = "planning_replies.rs"]
mod planning_replies;
#[path = "proposal.rs"]
mod proposal;
#[path = "review.rs"]
mod review;
#[path = "routing.rs"]
mod routing;
#[path = "strategy.rs"]
mod strategy;

pub(super) fn renders() -> Vec<(&'static str, String)> {
    [
        implementation::renders(),
        strategy::renders(),
        delivery::renders(),
        context::renders(),
        dependencies::renders(),
        planning::renders(),
        planning_replies::renders(),
        proposal::renders(),
        review::renders(),
        narrow_review::renders(),
        first_review::renders(),
        routing::renders(),
    ]
    .into_iter()
    .flatten()
    .collect()
}
