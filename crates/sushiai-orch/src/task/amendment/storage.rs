use super::{Amendment, PendingAmendment};
use crate::task::checks::authority::{CheckAuthority, Origin};
use crate::task::{Task, TaskData};

pub(crate) enum Settlement {
    Empty,
    Applied,
    Rejected,
}

impl Task {
    pub(crate) fn queue_final_checks(
        &mut self,
        authority: &CheckAuthority,
        checks: crate::task::checks::contribution::ChildChecks<'_>,
        now: i64,
    ) -> Result<(), String> {
        let mut pending = self.pending_amendment.clone().unwrap_or_else(|| {
            PendingAmendment::capture(self, Amendment::default(), Origin::Other)
        });
        let mut effective = pending.preview(self, authority, now)?;
        let before = effective.decisions.len();
        if checks.append_to(&mut effective, now) {
            pending
                .notes
                .extend(effective.decisions[before..].iter().cloned());
            pending.merge(
                Amendment {
                    final_verify: Some(effective.final_verify.clone()),
                    ..Default::default()
                },
                Origin::Other,
            );
        }
        self.data.pending_amendment = Some(pending);
        Ok(())
    }

    pub(crate) fn queue_pending_amendment(
        &mut self,
        amendment: Amendment,
        origin: Origin,
        authority: &CheckAuthority,
        now: i64,
    ) -> Result<(), String> {
        let effective = match &self.pending_amendment {
            Some(pending) => pending.preview(self, authority, now)?,
            None => self.clone(),
        };
        let mut checked = effective;
        amendment
            .clone()
            .apply_authorized(&mut checked, origin, authority, now)?;
        match self.data.pending_amendment.as_mut() {
            Some(pending) => pending.merge(amendment, origin),
            None => {
                self.data.pending_amendment =
                    Some(PendingAmendment::capture(self, amendment, origin))
            }
        }
        Ok(())
    }

    pub(crate) fn settle_pending_amendment(
        &mut self,
        authority: &CheckAuthority,
        now: i64,
    ) -> Settlement {
        let Some(pending) = self.pending_amendment.clone() else {
            return Settlement::Empty;
        };
        let result = match pending.preview(self, authority, now) {
            Ok(next) => {
                *self = next;
                Settlement::Applied
            }
            Err(why) => {
                self.record_decision(format!("Amendment not applied: {why}"));
                Settlement::Rejected
            }
        };
        self.data.pending_amendment = None;
        self.touch(now);
        result
    }

    pub(crate) fn retaining_pending_amendment(&self, stored: Option<&TaskData>) -> Task {
        let mut copy = self.clone();
        copy.data.pending_amendment = stored.and_then(|task| task.pending_amendment.clone());
        copy
    }

    pub(crate) fn adopt_pending_amendment(&mut self, stored: &TaskData) {
        self.data
            .pending_amendment
            .clone_from(&stored.pending_amendment);
    }

    pub(crate) fn carry_pending_amendment(&mut self, before: &Task) -> Result<(), String> {
        self.data.pending_amendment = before
            .pending_amendment
            .as_ref()
            .map(|pending| pending.carry_forward(before, self))
            .transpose()?;
        Ok(())
    }
}

impl TaskData {
    pub(crate) fn matches_pending_amendment(&self, expected: Option<&PendingAmendment>) -> bool {
        self.pending_amendment.as_ref() == expected
    }
}
