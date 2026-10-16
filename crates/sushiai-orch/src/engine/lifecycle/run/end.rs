use crate::engine::lifecycle::carry::attempt::CarryEnd;
use crate::engine::lifecycle::completion::Tail;
use crate::engine::lifecycle::evidence::validation::EvidenceEnd;
use crate::engine::lifecycle::inspection::InspectionEnd;
use crate::engine::lifecycle::native_result::NativeEnd;
use crate::engine::lifecycle::review_admission::AdmissionEnd;
use crate::policy::pipelines::task::{TaskExit, TaskFlow, TaskNext, TaskOutcome, TaskStep};
use crate::task::Failure;

pub(super) enum CycleEnd {
    Changed,
    Retry(u32),
    Stopped,
    Finished,
    Failed(Failure),
}

impl From<NativeEnd> for CycleEnd {
    fn from(end: NativeEnd) -> Self {
        match end {
            NativeEnd::Changed(_) => Self::Changed,
            NativeEnd::Missing => Self::Retry(0),
            NativeEnd::Stopped => Self::Stopped,
            NativeEnd::Failed(failure) => Self::Failed(failure),
        }
    }
}

impl From<InspectionEnd> for CycleEnd {
    fn from(end: InspectionEnd) -> Self {
        match end {
            InspectionEnd::Retry(extra) => Self::Retry(extra),
            InspectionEnd::Stopped => Self::Stopped,
            InspectionEnd::Failed(failure) => Self::Failed(failure),
        }
    }
}

impl From<CarryEnd> for CycleEnd {
    fn from(end: CarryEnd) -> Self {
        match end {
            CarryEnd::Tail(Tail::Continue) => Self::Retry(0),
            CarryEnd::Tail(Tail::Return) => Self::Finished,
            CarryEnd::Failed(failure) => Self::Failed(failure),
        }
    }
}

impl From<EvidenceEnd> for CycleEnd {
    fn from(end: EvidenceEnd) -> Self {
        match end {
            EvidenceEnd::Stopped => Self::Stopped,
            EvidenceEnd::Failed(failure) => Self::Failed(failure),
        }
    }
}

impl From<AdmissionEnd> for CycleEnd {
    fn from(end: AdmissionEnd) -> Self {
        match end {
            AdmissionEnd::Changed => Self::Changed,
            AdmissionEnd::Stopped => Self::Stopped,
            AdmissionEnd::Failed(failure) => Self::Failed(failure),
        }
    }
}

impl CycleEnd {
    fn outcome(&self) -> TaskOutcome {
        match self {
            Self::Changed => TaskOutcome::Changed,
            Self::Retry(_) => TaskOutcome::Retry,
            Self::Stopped => TaskOutcome::Stopped,
            Self::Finished => TaskOutcome::Finished,
            Self::Failed(_) => TaskOutcome::Failed,
        }
    }

    pub(super) fn route(self, flow: &TaskFlow, from: TaskStep) -> Self {
        match (flow.next(from, self.outcome()), self) {
            (TaskNext::Exit(TaskExit::Retry), Self::Retry(extra)) => Self::Retry(extra),
            (TaskNext::Exit(TaskExit::Retry), Self::Changed) => Self::Changed,
            (TaskNext::Exit(TaskExit::Stop), Self::Stopped) => Self::Stopped,
            (TaskNext::Exit(TaskExit::Finish), Self::Finished) => Self::Finished,
            (TaskNext::Exit(TaskExit::Fail), Self::Failed(failure)) => Self::Failed(failure),
            _ => unreachable!("a terminal operation cannot produce a later operation's input"),
        }
    }
}

pub(super) fn route_tail(flow: &TaskFlow, tail: Tail) -> Tail {
    let outcome = match tail {
        Tail::Continue => TaskOutcome::Continue,
        Tail::Return => TaskOutcome::Finished,
    };
    match flow.next(TaskStep::Complete, outcome) {
        TaskNext::Exit(TaskExit::Retry) => Tail::Continue,
        TaskNext::Exit(TaskExit::Finish) => Tail::Return,
        _ => unreachable!("completion has no unfinished receipt"),
    }
}
