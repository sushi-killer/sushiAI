use crate::task::{AnswerSource, AnsweredQuestion, ParkRecord, Task, TaskStatus};

pub(crate) struct Cancellation {
    park: Option<ParkRecord>,
    explicit: bool,
    line: String,
    last_answer: Option<AnsweredQuestion>,
    pub(crate) restart: bool,
    pub(crate) restart_children: bool,
}

impl Cancellation {
    pub(crate) fn apply(&self, task: &mut Task) {
        let task = &mut task.data;
        task.queue.park = if self.restart {
            None
        } else {
            self.park.clone()
        };
        task.queue.stopped_explicitly = self.explicit && !self.restart;
        task.status = if self.restart || task.queue.park.is_some() {
            TaskStatus::Queued
        } else {
            TaskStatus::Stopped
        };
        if !task.decisions.contains(&self.line) {
            task.decisions.push(self.line.clone());
        }
        if let Some(p) = &self.park {
            let note = format!("Park: {} (by {})", p.reason, p.by.label());
            if !task.decisions.contains(&note) {
                task.decisions.push(note);
            }
        }
        if let Some(answer) = &self.last_answer {
            if !task.question_history.contains(answer) {
                task.question_history.push(answer.clone());
            }
        }
        task.queue.restart_children = self.restart_children;
        task.question = None;
    }
}

impl AnswerSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Owner => "Owner",
            Self::Lead => "Lead",
            Self::Chat => "orchestrator chat",
            Self::Mcp => "MCP caller",
            Self::Overseer => "overseer",
            Self::Policy => "policy",
        }
    }

    pub(crate) fn control_line(self, action: &str, reason: Option<&str>) -> String {
        let origin = match self {
            Self::Owner => format!("Owner: {action} (UI)"),
            Self::Lead => format!("Lead: {action} (RPC)"),
            source => format!("Orchestrator: {action} (requested by {})", source.label()),
        };
        match reason.map(str::trim).filter(|r| !r.is_empty()) {
            Some(reason) => format!("{origin}: {reason}"),
            None => origin,
        }
    }
}

impl Cancellation {
    pub(crate) fn new(
        park: Option<ParkRecord>,
        explicit: bool,
        line: String,
        last_answer: Option<AnsweredQuestion>,
    ) -> Self {
        Self {
            park,
            explicit,
            line,
            last_answer,
            restart: false,
            restart_children: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::test_support::sample_task;
    use crate::task::{AskedBy, AttemptStatus, Question, QuestionKind};

    #[test]
    fn repeated_cancellation_keeps_one_decision_and_restart_releases_the_park() {
        let mut task = sample_task("park", TaskStatus::Waiting, AttemptStatus::Running);
        let park = ParkRecord {
            reason: "A synthetic pause".into(),
            until: None,
            by: AnswerSource::Owner,
        };
        let mut cancellation =
            Cancellation::new(Some(park.clone()), true, "Owner: parked".into(), None);
        task.ask(
            Question::new(
                "Continue?",
                vec![],
                QuestionKind::AgentQuestion,
                AskedBy::Implement,
                1,
            ),
            2,
        );
        cancellation.apply(&mut task);
        cancellation.apply(&mut task);
        assert_eq!(task.status, TaskStatus::Queued);
        assert!(task.question.is_none());
        assert_eq!(task.queue.park, Some(park));
        assert!(task.queue.stopped_explicitly);
        assert_eq!(task.decisions.len(), 2);
        cancellation.restart = true;
        cancellation.restart_children = true;
        cancellation.apply(&mut task);
        assert!(task.queue.park.is_none() && !task.queue.stopped_explicitly);
        assert!(task.queue.restart_children);
        assert_eq!(task.decisions.len(), 2);
    }
}
