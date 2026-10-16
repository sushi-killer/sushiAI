use crate::task::checks::types::{CheckSource, SourcedCheck};
use crate::task::state::{Task, TaskData};

impl TaskData {
    /// The commands the planner wrote for this task, each with whether it
    /// sits in verify or finalVerify: those, the checks, the held-out check
    /// and the `-- check:` part of each criterion.
    pub fn planner_check_commands(&self) -> Vec<(String, bool)> {
        self.verify
            .iter()
            .chain(&self.final_verify)
            .cloned()
            .map(|c| (c, true))
            .chain(
                self.checks
                    .iter()
                    .chain(self.held_out.as_ref())
                    .map(|c| (c.run.clone(), false)),
            )
            .chain(
                self.criteria
                    .iter()
                    .filter_map(|c| c.split_once("-- check:"))
                    .map(|(_, command)| (command.trim().to_string(), false)),
            )
            .collect()
    }
}

impl Task {
    /// Records the source of each command now in the plan. `classify` rules
    /// on one command (and whether it sits in verify or finalVerify); a
    /// command in several places gets the stricter source, the first that is
    /// not `Planner`. A command from a setting or a preset is never
    /// recorded, so it counts as the repo's.
    pub fn record_check_sources(&mut self, sources: Vec<SourcedCheck>) {
        for source in sources {
            record_source(&mut self.data, source);
        }
        self.data.planner_tests.legacy_commands.clear();
    }
}

pub(crate) fn record_source(task: &mut TaskData, source: SourcedCheck) {
    let rank = |source| match source {
        CheckSource::Planner => 0,
        CheckSource::Repo => 1,
        CheckSource::Owner => 2,
    };
    match task.planner_tests.sources.iter_mut().find(|known| {
        super::commands::flat_command(&known.command)
            == super::commands::flat_command(&source.command)
    }) {
        Some(known) if rank(source.source) > rank(known.source) => known.source = source.source,
        Some(_) => {}
        None => task.planner_tests.sources.push(source),
    }
}

pub(crate) fn migrate_sources(task: &mut Task, sources: Vec<SourcedCheck>) {
    let task = &mut task.data;
    let legacy = std::mem::take(&mut task.planner_tests.legacy_commands);
    for source in sources
        .into_iter()
        .filter(|source| legacy.contains(&source.command))
    {
        record_source(task, source);
    }
    task.planner_tests
        .sources
        .retain(|source| legacy.contains(&source.command));
}
