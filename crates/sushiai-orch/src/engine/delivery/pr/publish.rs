use super::{
    automatic_carry::carry_before_draft, plan::draft_plan, remote::remote_available,
    retry::publish_plan,
};
use crate::engine::delivery::pr::pending::owed;
use crate::engine::delivery::pr::pending::record;
use crate::engine::state::App;
use std::sync::Arc;

pub(in crate::engine) async fn open_pr_on_done(app: &Arc<App>, id: &str) {
    let Some(mut task) = owed(app, id) else {
        return record(app, id, Vec::new(), 0.0, true);
    };
    if !remote_available(app, &task).await {
        return;
    }
    if !carry_before_draft(app, &task).await {
        return;
    }
    let Some(fresh) = owed(app, id) else {
        return record(app, id, Vec::new(), 0.0, true);
    };
    task = fresh;
    let Some(plan) = draft_plan(app, &task).await else {
        return;
    };
    publish_plan(app, &task, &plan).await;
}
