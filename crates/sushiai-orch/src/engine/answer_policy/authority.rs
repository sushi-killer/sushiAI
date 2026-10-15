use crate::engine::verification::scoping::scoped_checks_for;
use crate::policy::paths::glob_match;
use crate::policy::settings::Settings;
use crate::task::amendment::Amendment;
use crate::task::checks::authority::Origin;
use crate::task::checks::commands::flat_command;
use crate::task::Task;

pub(in crate::engine) fn is_shared_check(settings: &Settings, task: &Task, command: &str) -> bool {
    check_authority(settings, task).is_shared(task, command)
}

pub(in crate::engine) fn check_authority(
    settings: &Settings,
    task: &Task,
) -> crate::task::checks::authority::CheckAuthority {
    crate::task::checks::authority::CheckAuthority {
        named: task
            .planner_check_commands()
            .into_iter()
            .map(|(command, _)| command)
            .chain(
                task.planner_tests
                    .sources
                    .iter()
                    .map(|source| source.command.clone()),
            )
            .filter(|command| {
                let command = flat_command(command);
                scoped_checks_for(settings, task).iter().any(|e| {
                    e.repo
                        .as_deref()
                        .is_none_or(|r| crate::git::paths::same_repo_path(r, &task.repo))
                        && (glob_match(&e.command, &command) || flat_command(&e.command) == command)
                }) || settings.after_land.iter().any(|a| {
                    crate::git::paths::same_repo_path(&a.repo, &task.repo)
                        && flat_command(&a.run) == command
                })
            })
            .collect(),
    }
}

pub(in crate::engine) fn removal_refusal(
    settings: &Settings,
    task: &Task,
    commands: &[String],
    origin: Origin,
) -> Option<String> {
    crate::task::checks::authority::removal_refusal(
        &check_authority(settings, task),
        task,
        commands,
        origin,
    )
}

pub(in crate::engine) fn criterion_text_refusal(
    settings: &Settings,
    task: &Task,
    i: usize,
    new_text: &str,
) -> Option<String> {
    crate::task::checks::authority::criterion_text_refusal(
        &check_authority(settings, task),
        task,
        i,
        new_text,
    )
}

pub(in crate::engine) fn remove_check_command(
    settings: &Settings,
    task: &mut Task,
    command: &str,
    origin: Origin,
) -> Result<(), String> {
    crate::task::checks::authority::remove_check_command(
        &check_authority(settings, task),
        task,
        command,
        origin,
    )
}

pub(in crate::engine) fn shared_check_amend_refusal(
    settings: &Settings,
    task: &Task,
    amendment: &Amendment,
) -> Option<String> {
    crate::task::checks::authority::shared_check_amend_refusal(
        &check_authority(settings, task),
        task,
        amendment,
    )
}
