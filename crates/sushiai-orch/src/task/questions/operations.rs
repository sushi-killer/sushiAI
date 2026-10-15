use super::{WAIVED, WAIVER_BY};
use crate::task::{AnsweredQuestion, Assumption, Question, QuestionKind, Task, TaskStatus};

impl Task {
    pub(crate) fn ask(&mut self, question: Question, now: i64) {
        self.data.question = Some(question);
        self.data.status = TaskStatus::Waiting;
        self.data.updated_at = now;
    }
    pub(crate) fn clear_question(&mut self) {
        self.data.question = None;
    }
    pub(crate) fn take_question(&mut self) -> Option<Question> {
        self.data.question.take()
    }
    pub(crate) fn resume_phase(&mut self, status: TaskStatus, now: i64) {
        self.data.status = status;
        self.data.updated_at = now;
    }
    pub(crate) fn merge_question_delivery(&mut self, saved: &Question) {
        if let Some(q) = self
            .data
            .question
            .as_mut()
            .filter(|q| q.asked_at == saved.asked_at)
        {
            q.held_for_overseer = saved.held_for_overseer;
            q.held_at = saved.held_at;
            q.for_owner |= saved.for_owner;
        }
    }
    pub(crate) fn hold_question(&mut self, at: i64) -> bool {
        let Some(q) = self.data.question.as_mut() else {
            return false;
        };
        q.held_for_overseer = true;
        q.held_at = Some(at);
        true
    }
    pub(crate) fn release_question_hold(&mut self, hand_over: bool) -> bool {
        let Some(q) = self.data.question.as_mut() else {
            return false;
        };
        q.release();
        q.for_owner |= hand_over;
        true
    }
    pub(crate) fn hand_question_to_owner(&mut self, asked_at: Option<i64>, now: i64) -> bool {
        if self.status != TaskStatus::Waiting {
            return false;
        }
        let Some(q) = self.data.question.as_mut().filter(|q| {
            !q.for_owner && q.kind != QuestionKind::AttemptsFailing && q.asked_at == asked_at
        }) else {
            return false;
        };
        q.for_owner = true;
        self.data.updated_at = now;
        true
    }
    pub(crate) fn hand_over_question(&mut self, now: i64) {
        if let Some(q) = self.data.question.as_mut() {
            q.release();
            q.for_owner = true;
            self.data.updated_at = now;
        }
    }
    pub(crate) fn mark_overseer_woken(&mut self) {
        self.data.overseer_woken = true;
    }
    pub(crate) fn adopt_answer_receipt(&mut self, saved: &crate::task::TaskData) {
        self.data
            .question_history
            .clone_from(&saved.question_history);
        self.data.assumptions.clone_from(&saved.assumptions);
        self.data.question.clone_from(&saved.question);
        self.data.status = saved.status;
        self.data.updated_at = saved.updated_at;
        match saved.question_history.last().map(|answer| answer.kind) {
            Some(QuestionKind::Impossible) => self
                .data
                .loop_state
                .impossible_answers
                .clone_from(&saved.loop_state.impossible_answers),
            Some(QuestionKind::AttemptsFailing) => {
                self.data.queue.rate_pauses = saved.queue.rate_pauses
            }
            _ => {}
        }
    }
    pub(crate) fn record_answer_history(&mut self, answer: AnsweredQuestion) {
        self.data.question_history.push(answer);
    }
    pub(crate) fn record_assumption(&mut self, assumption: Assumption) {
        self.data.assumptions.push(assumption);
    }
    pub(crate) fn overturn_assumption(&mut self, index: usize, answer: String) {
        let Some(assumption) = self.data.assumptions.get_mut(index) else {
            return;
        };
        assumption.overturned = true;
        assumption.owner_answer = Some(answer);
        if assumption.kind == Some(QuestionKind::PreexistingFailure)
            && assumption.by == WAIVER_BY
            && assumption.answer == WAIVED
        {
            let commands = assumption.commands.clone();
            for command in commands {
                self.record_kept_check(command);
            }
        }
    }
    pub(crate) fn replace_last_decision(&mut self, line: String) {
        if let Some(last) = self.data.decisions.last_mut() {
            *last = line;
        }
    }
    pub(crate) fn remember_permission(&mut self, name: String) {
        if !self.loop_state.answered_permissions.contains(&name) {
            self.data.loop_state.answered_permissions.push(name);
        }
    }
    pub(crate) fn count_non_owner_continue(&mut self) {
        self.data.loop_state.non_owner_continues += 1;
    }
    pub(crate) fn remember_impossible_answer(&mut self, criterion: Option<usize>) {
        self.data.loop_state.impossible_answers.extend(criterion);
    }
}

#[cfg(test)]
mod tests {
    use crate::task::test_support::sample_task;
    use crate::task::{AskedBy, AttemptStatus, Question, QuestionKind, TaskStatus};

    fn question(at: i64) -> Question {
        Question::new(
            "A synthetic choice",
            vec!["continue".into()],
            QuestionKind::AgentQuestion,
            AskedBy::Implement,
            at,
        )
    }

    #[test]
    fn delivery_observations_only_merge_into_the_same_question_and_keep_owner_handoff() {
        let mut task = sample_task("delivery", TaskStatus::Running, AttemptStatus::Running);
        task.ask(question(20), 21);
        let mut stale = question(10);
        stale.for_owner = true;
        task.merge_question_delivery(&stale);
        assert!(!task.question.as_ref().unwrap().for_owner);
        let mut current = question(20);
        current.for_owner = true;
        current.held_for_overseer = true;
        current.held_at = Some(22);
        task.merge_question_delivery(&current);
        task.release_question_hold(false);
        let q = task.question.as_ref().unwrap();
        assert!(q.for_owner && !q.held_for_overseer && q.held_at.is_none());
        assert_eq!(task.updated_at, 21);
    }

    #[test]
    fn owner_handoff_rejects_replaced_questions_and_the_failure_ladder() {
        let mut task = sample_task("handoff", TaskStatus::Running, AttemptStatus::Running);
        task.ask(question(10), 11);
        assert!(!task.hand_question_to_owner(Some(9), 12));
        assert!(task.hand_question_to_owner(Some(10), 13));
        assert!(!task.hand_question_to_owner(Some(10), 14));
        let mut ladder = question(20);
        ladder.kind = QuestionKind::AttemptsFailing;
        task.ask(ladder, 21);
        assert!(!task.hand_question_to_owner(Some(20), 22));
        assert!(!task.question.as_ref().unwrap().for_owner);
    }
}

#[cfg(test)]
mod overturn;
