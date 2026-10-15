use crate::engine::attempts::observations::note_first_edit;
use crate::engine::persistence::persist_attempt_observation;
use crate::engine::state::App;
use crate::harness;
use crate::harness::native::error::RunError;
use crate::policy::routing::Harness;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct RunStream {
    pub(super) outcome: harness::RunOutcome,
    stderr_tail: String,
    session_persisted: bool,
    edit_seen: bool,
}

impl RunStream {
    pub(super) async fn observe_attempt(
        &mut self,
        app: &Arc<App>,
        task_id: &str,
        attempt_n: u32,
        line: &str,
    ) {
        if !self.session_persisted {
            if let Some(sid) = self.outcome.session_id.clone() {
                self.session_persisted = true;
                persist_attempt_observation(
                    app,
                    task_id,
                    attempt_n,
                    crate::task::attempts::RunObservation::Session(sid),
                )
                .await;
            }
        }
        if !self.edit_seen {
            self.edit_seen = note_first_edit(app, task_id, attempt_n, line).await;
        }
    }

    pub(super) fn stderr(&mut self, line: &str) {
        self.stderr_tail.push_str(line);
        self.stderr_tail.push('\n');
        if self.stderr_tail.len() > 4000 {
            let cut = self.stderr_tail.len() - 4000;
            let cut = (cut..self.stderr_tail.len())
                .find(|i| self.stderr_tail.is_char_boundary(*i))
                .unwrap_or(0);
            self.stderr_tail.drain(..cut);
        }
    }

    pub(super) fn exit(
        &mut self,
        harness: Harness,
        tracked: bool,
        status: std::io::Result<std::process::ExitStatus>,
    ) -> Result<(), RunError> {
        if let Ok(status) = status {
            if tracked && matches!(status.code(), Some(126 | 127)) {
                let tail = self.stderr_tail.trim();
                return Err(RunError::NotFound(if tail.is_empty() {
                    format!("{harness:?}: command not found")
                } else {
                    tail.to_string()
                }));
            }
            if !status.success() && self.outcome.error.is_none() {
                let tail = self.stderr_tail.trim();
                self.outcome.error = Some(if tail.is_empty() {
                    harness::exit_error(harness, status)
                } else {
                    tail.to_string()
                });
            }
        }
        Ok(())
    }
}
