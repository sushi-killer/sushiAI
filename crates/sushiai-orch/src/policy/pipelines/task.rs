use serde::Deserialize;
use std::collections::BTreeSet;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TaskStep {
    Execute,
    Settle,
    Inspect,
    Qualify,
    Review,
    Complete,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TaskOutcome {
    Ready,
    Changed,
    Retry,
    Stopped,
    Failed,
    Finished,
    Continue,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TaskExit {
    Retry,
    Stop,
    Finish,
    Fail,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub(crate) enum TaskNext {
    Step(TaskStep),
    Exit(TaskExit),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transition {
    from: TaskStep,
    on: TaskOutcome,
    to: TaskNext,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskFlow {
    entry: TaskStep,
    transitions: Vec<Transition>,
}

pub(crate) fn compiled() -> &'static TaskFlow {
    static FLOW: OnceLock<TaskFlow> = OnceLock::new();
    FLOW.get_or_init(|| {
        TaskFlow::parse(include_str!("../../../pipelines/task.yaml"))
            .expect("pipelines/task.yaml is valid")
    })
}

impl TaskFlow {
    fn parse(text: &str) -> Result<Self, String> {
        let flow: Self = super::decode(text)?;
        if flow.entry != TaskStep::Execute {
            return Err("task flow must start with an unexecuted attempt".into());
        }
        let mut seen = BTreeSet::new();
        for edge in &flow.transitions {
            if !edge.from.outcomes().contains(&edge.on) || !edge.to.accepts(edge.from, edge.on) {
                return Err(format!("incompatible task transition: {edge:?}"));
            }
            if !seen.insert((edge.from, edge.on)) {
                return Err(format!("duplicate task transition: {edge:?}"));
            }
        }
        for step in TaskStep::ALL {
            for outcome in step.outcomes() {
                if !seen.contains(&(step, *outcome)) {
                    return Err(format!("missing task transition: {step:?}/{outcome:?}"));
                }
            }
        }
        Ok(flow)
    }

    pub(crate) fn entry(&self) -> TaskStep {
        self.entry
    }

    pub(crate) fn next(&self, from: TaskStep, on: TaskOutcome) -> TaskNext {
        self.transitions
            .iter()
            .find(|edge| edge.from == from && edge.on == on)
            .expect("validated task outcome has a transition")
            .to
    }
}

impl TaskStep {
    const ALL: [Self; 6] = [
        Self::Execute,
        Self::Settle,
        Self::Inspect,
        Self::Qualify,
        Self::Review,
        Self::Complete,
    ];

    fn outcomes(self) -> &'static [TaskOutcome] {
        use TaskOutcome::{Changed, Continue, Failed, Finished, Ready, Retry, Stopped};
        match self {
            Self::Execute => &[Ready, Retry, Finished],
            Self::Settle => &[Ready, Changed, Retry, Stopped, Failed],
            Self::Inspect => &[Ready, Retry, Stopped, Failed],
            Self::Qualify => &[Ready, Retry, Changed, Stopped, Finished, Failed],
            Self::Review => &[Ready, Changed, Stopped, Failed],
            Self::Complete => &[Continue, Finished],
        }
    }
}

impl TaskNext {
    fn accepts(self, from: TaskStep, outcome: TaskOutcome) -> bool {
        match (self, outcome) {
            (Self::Step(to), TaskOutcome::Ready) => matches!(
                (from, to),
                (TaskStep::Execute, TaskStep::Settle)
                    | (TaskStep::Settle, TaskStep::Inspect)
                    | (TaskStep::Inspect, TaskStep::Qualify)
                    | (TaskStep::Qualify, TaskStep::Review)
                    | (TaskStep::Review, TaskStep::Complete)
            ),
            (
                Self::Exit(TaskExit::Retry),
                TaskOutcome::Retry | TaskOutcome::Changed | TaskOutcome::Continue,
            )
            | (Self::Exit(TaskExit::Stop), TaskOutcome::Stopped)
            | (Self::Exit(TaskExit::Finish), TaskOutcome::Finished)
            | (Self::Exit(TaskExit::Fail), TaskOutcome::Failed) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests;
