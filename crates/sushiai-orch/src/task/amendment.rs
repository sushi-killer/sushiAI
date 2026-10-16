use crate::task::{check_part_command, valid_check, valid_checks, Check, Task, TaskData};

/// What `task.amend` changes; a `None` field is left as it is.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Amendment {
    pub criteria: Option<Vec<String>>,
    /// The `criteria` texts flagged visual; only meaningful when `criteria` is set.
    pub visual_criteria: Vec<String>,
    pub verify: Option<Vec<String>>,
    pub final_verify: Option<Vec<String>>,
    /// The screenshot command; an empty string removes it.
    pub screenshot: Option<String>,
    pub checks: Option<Vec<Check>>,
    #[serde(with = "held_out_edit")]
    pub held_out: Option<Option<Check>>,
    pub depends_on: Option<Vec<String>>,
    pub dependency_titles: Vec<String>,
    /// Free-text supplements appended to the task's `addenda`, in order.
    #[serde(default)]
    pub addenda: Vec<String>,
}

impl Amendment {
    pub fn is_empty(&self) -> bool {
        self.fields().is_empty()
    }

    /// The camelCase names of the fields it changes, in a fixed order.
    pub fn fields(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.criteria.is_some() {
            names.push("criteria");
        }
        if self.verify.is_some() {
            names.push("verify");
        }
        if self.final_verify.is_some() {
            names.push("finalVerify");
        }
        if self.screenshot.is_some() {
            names.push("screenshot");
        }
        if self.checks.is_some() {
            names.push("checks");
        }
        if self.held_out.is_some() {
            names.push("heldOut");
        }
        if self.depends_on.is_some() {
            names.push("dependsOn");
        }
        if !self.addenda.is_empty() {
            names.push("addendum");
        }
        names
    }

    /// A later amendment wins field by field; addenda accumulate in order.
    pub fn merge(&mut self, later: Amendment) {
        if later.criteria.is_some() {
            self.criteria = later.criteria;
            self.visual_criteria = later.visual_criteria;
        }
        self.verify = later.verify.or(self.verify.take());
        self.final_verify = later.final_verify.or(self.final_verify.take());
        self.screenshot = later.screenshot.or(self.screenshot.take());
        self.checks = later.checks.or(self.checks.take());
        self.held_out = later.held_out.or(self.held_out.take());
        if later.depends_on.is_some() {
            self.depends_on = later.depends_on;
            self.dependency_titles = later.dependency_titles;
        }
        self.addenda.extend(later.addenda);
    }

    /// The criteria whose check this amendment rebuilds from their new
    /// `-- check:` command, with that command: changed or new criteria whose
    /// text names a runnable command, when `checks` is not set. A criterion
    /// whose one check already runs that command keeps it, baseline and all.
    pub(crate) fn rebuilds(&self, task: &TaskData) -> Vec<(usize, String)> {
        let (Some(new), None) = (&self.criteria, &self.checks) else {
            return Vec::new();
        };
        new.iter()
            .enumerate()
            .filter(|(i, text)| task.criteria.get(*i) != Some(*text))
            .filter_map(|(i, text)| check_part_command(text).map(|command| (i, command)))
            .filter(|(i, command)| {
                let mut own = task.checks.iter().filter(|c| c.criterion == *i);
                !matches!((own.next(), own.next()), (Some(c), None) if &c.run == command)
            })
            .collect()
    }

    pub(crate) fn apply_authorized(
        self,
        task: &mut Task,
        origin: crate::task::checks::authority::Origin,
        authority: &crate::task::checks::authority::CheckAuthority,
        now: i64,
    ) -> Result<(), String> {
        let task = &mut task.data;
        ensure_amendable(task)?;
        self.validate_indices(task)?;
        if origin == crate::task::checks::authority::Origin::Other {
            if let Some(why) =
                crate::task::checks::authority::shared_check_amend_refusal(authority, task, &self)
            {
                return Err(why);
            }
        }
        self.apply(task, now);
        Ok(())
    }

    pub(crate) fn validate_indices(&self, task: &TaskData) -> Result<(), String> {
        let len = self.criteria.as_ref().map_or(task.criteria.len(), Vec::len);
        let retained = if self.criteria.is_some() && self.checks.is_none() {
            task.checks.iter().find(|check| check.criterion >= len)
        } else {
            None
        };
        let held = match &self.held_out {
            Some(check) => check.as_ref(),
            None if self.criteria.is_some() => task.held_out.as_ref(),
            None => None,
        };
        if let Some(check) = self
            .checks
            .iter()
            .flatten()
            .chain(retained)
            .chain(held)
            .find(|check| check.criterion >= len)
        {
            return Err(format!(
                "check criterion {} is out of range: there are {len} criteria",
                check.criterion
            ));
        }
        Ok(())
    }

    /// Replaces the fields and records their names, plus dependency titles.
    fn apply(self, task: &mut TaskData, now: i64) {
        let line = format!("Amended: {}", self.fields().join(", "));
        let rebuilds = self.rebuilds(task);
        if let Some(v) = self.criteria {
            task.criteria = v;
            task.visual_criteria = self.visual_criteria;
        }
        if let Some(v) = self.verify {
            task.verify = v;
        }
        if let Some(v) = self.final_verify {
            task.final_verify = v;
        }
        if let Some(v) = self.screenshot {
            let v = v.trim();
            task.brief_check.screenshot = (!v.is_empty()).then(|| v.to_string());
        }
        let len = task.criteria.len();
        if let Some(v) = self.checks {
            task.checks = valid_checks(v, len);
        }
        if let Some(h) = self.held_out {
            task.held_out = valid_check(h, len);
        }
        task.addenda.extend(self.addenda);
        task.decisions.push(line);
        if !rebuilds.is_empty() {
            for (i, command) in &rebuilds {
                // A check that already runs the command keeps its baseline.
                let kept = task
                    .checks
                    .iter()
                    .find(|c| c.criterion == *i && &c.run == command)
                    .cloned();
                task.checks.retain(|c| c.criterion != *i);
                task.checks.push(kept.unwrap_or(Check {
                    criterion: *i,
                    run: command.clone(),
                    baseline: None,
                }));
            }
            task.checks.sort_by_key(|c| c.criterion);
            let names: Vec<String> = rebuilds.iter().map(|(i, _)| i.to_string()).collect();
            task.decisions.push(format!(
                "Orchestrator: rebuilt the check of criterion {} from its `-- check:`",
                names.join(", ")
            ));
        }
        if let Some(deps) = self.depends_on {
            task.depends_on = deps;
            let waits = if self.dependency_titles.is_empty() {
                "nothing".to_string()
            } else {
                self.dependency_titles
                    .iter()
                    .map(|title| format!("\"{title}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            task.decisions
                .push(format!("Dependencies: waits for {waits}"));
        }
        task.updated_at = now;
    }
}

/// Live-loop handoff preserves each caller's authority and the original
/// content identity while retaining the existing field-wise merge behavior.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PendingAmendment {
    snapshot: crate::task::checks::snapshot::CheckSnapshot,
    edits: Vec<(Amendment, crate::task::checks::authority::Origin)>,
    merged: Amendment,
    notes: Vec<String>,
}

impl std::ops::Deref for PendingAmendment {
    type Target = Amendment;
    fn deref(&self) -> &Amendment {
        &self.merged
    }
}

impl PendingAmendment {
    pub(crate) fn capture(
        task: &Task,
        amendment: Amendment,
        origin: crate::task::checks::authority::Origin,
    ) -> Self {
        Self {
            snapshot: crate::task::checks::snapshot::CheckSnapshot::capture(task),
            merged: amendment.clone(),
            edits: vec![(amendment, origin)],
            notes: Vec::new(),
        }
    }

    pub(crate) fn carry_forward(&self, before: &Task, after: &Task) -> Result<Self, String> {
        if !self.snapshot.matches(before) || !self.snapshot.same_contract_after_carry(after) {
            return Err("the pending amendment is stale".to_string());
        }
        let mut carried = self.clone();
        carried.snapshot = crate::task::checks::snapshot::CheckSnapshot::capture(after);
        Ok(carried)
    }

    pub(crate) fn merge(
        &mut self,
        later: Amendment,
        origin: crate::task::checks::authority::Origin,
    ) {
        self.merged.merge(later.clone());
        self.edits.push((later, origin));
    }

    pub(crate) fn preview(
        &self,
        task: &Task,
        authority: &crate::task::checks::authority::CheckAuthority,
        now: i64,
    ) -> Result<Task, String> {
        if !self.snapshot.matches(task) {
            return Err("the pending amendment is stale".to_string());
        }
        let mut effective = task.clone();
        for (edit, origin) in &self.edits {
            if *origin == crate::task::checks::authority::Origin::Other {
                if let Some(why) = crate::task::checks::authority::shared_check_amend_refusal(
                    authority, &effective, edit,
                ) {
                    return Err(why);
                }
            }
            edit.clone()
                .apply_authorized(&mut effective, *origin, authority, now)?;
        }
        // Store the same single field-wise merge as before the handoff.
        let mut result = task.clone();
        self.merged.clone().apply(&mut result.data, now);
        result.record_decisions(self.notes.clone());
        Ok(result)
    }
}

pub(crate) fn ensure_amendable(task: &crate::task::TaskData) -> Result<(), String> {
    if task.archived {
        return Err("task is archived".to_string());
    }
    if !matches!(
        task.status,
        crate::task::TaskStatus::Drafting
            | crate::task::TaskStatus::Running
            | crate::task::TaskStatus::Waiting
            | crate::task::TaskStatus::Queued
            | crate::task::TaskStatus::Stopped
            | crate::task::TaskStatus::Failed
    ) {
        return Err(format!("task is {:?}: only drafting, running, waiting, queued, stopped or failed tasks can be amended", task.status).to_lowercase());
    }
    Ok(())
}

mod held_out_edit {
    use crate::task::Check;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<S: Serializer>(
        value: &Option<Option<Check>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.as_ref().map(|check| (check,)).serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Option<Check>>, D::Error> {
        Option::<(Option<Check>,)>::deserialize(deserializer)
            .map(|value| value.map(|(check,)| check))
    }
}

pub(crate) mod storage;
