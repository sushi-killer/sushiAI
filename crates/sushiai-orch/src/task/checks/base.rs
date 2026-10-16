use crate::task::{BaseAnswer, Task};

impl Task {
    pub(crate) fn record_kept_check(&mut self, command: String) {
        if !self.data.loop_state.kept_checks.contains(&command) {
            self.data.loop_state.kept_checks.push(command);
        }
    }
    pub(crate) fn record_base_answer(&mut self, answer: BaseAnswer) {
        self.data.loop_state.base_answers.push(answer);
    }
}
