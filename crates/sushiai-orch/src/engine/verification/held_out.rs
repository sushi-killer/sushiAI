use crate::engine::verification::runner::run_verify_commands;
use crate::engine::verification::runner::CheckRunner;
use crate::task::attempt::VerifyOutcome;
use crate::task::checks::types::Check;
use crate::task::state::Task;

/// Runs the held-out check and returns its outcome with the command scrubbed
/// out of the command field and the tail, plus the failure detail when it
/// did not exit 0.
pub(in crate::engine) async fn run_held_out(
    runner: CheckRunner<'_>,
    task: &Task,
    check: &Check,
) -> (VerifyOutcome, Option<String>) {
    let CheckRunner {
        ctx,
        cwd: worktree,
        sandbox,
        cancel,
        ..
    } = runner;
    let label = crate::task::checks::commands::held_out_label(check.criterion);
    let ctx = ctx.with_label(&label);
    let result = run_verify_commands(
        crate::engine::verification::runner::CheckRunner::from_context(
            ctx,
            worktree,
            sandbox,
            &task.base_sha,
            cancel,
        ),
        std::slice::from_ref(&check.run),
    )
    .await
    .remove(0);
    held_out_result(task, check, result)
}

/// Runs a held-out gate using its real command as the key and its label in decisions.
#[cfg(test)]
pub(in crate::engine) async fn run_held_out_cached(
    runner: &CheckRunner<'_>,
    task: &Task,
    check: &Check,
) -> (VerifyOutcome, Option<String>, Vec<String>) {
    let (result, lines) = runner
        .run_one(
            &check.run,
            &crate::task::checks::commands::held_out_label(check.criterion),
            None,
        )
        .await;
    let (result, detail) = held_out_result(task, check, result);
    (result, detail, lines)
}

fn held_out_result(
    task: &Task,
    check: &Check,
    mut result: VerifyOutcome,
) -> (VerifyOutcome, Option<String>) {
    result.command = crate::task::checks::commands::held_out_label(check.criterion);
    result.tail = result.tail.replace(&check.run, "<held-out check>");
    let detail = (result.code != Some(0)).then(|| {
        format!(
            "Held-out check for criterion {} failed: {}. exited {}.\n{}",
            check.criterion,
            task.criteria
                .get(check.criterion)
                .map(String::as_str)
                .unwrap_or(""),
            result
                .code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "null".to_string()),
            result.tail
        )
        .replace(&check.run, "<held-out check>")
    });
    (result, detail)
}
