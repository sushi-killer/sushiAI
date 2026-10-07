//! The events the engine broadcasts. The daemon module forwards each one, serialized, as an
//! `orch.event` notification; clients tell them apart by the `event` tag.

use crate::model::{Message, Task};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event")]
pub enum Event {
    #[serde(rename = "task")]
    Task { task: Box<Task> },
    #[serde(rename = "log")]
    Log {
        #[serde(rename = "taskId")]
        task_id: String,
        attempt: u32,
        line: String,
    },
    /// A repo's current orchestrator chat session, whole, after any change,
    /// with the id of that session and a light summary of all of them.
    #[serde(rename = "chat")]
    Chat {
        thread: Box<serde_json::Value>,
        current: String,
        sessions: Box<serde_json::Value>,
    },
    /// An agent-to-agent message, when it is sent and again when delivered.
    #[serde(rename = "message")]
    Message { message: Box<Message> },
    /// A `repo.audit` run, as `repo.audit.get` returns it, when it starts
    /// and when it ends; its progress lines are `log` events keyed by the
    /// audit id.
    #[serde(rename = "audit")]
    Audit { audit: Box<serde_json::Value> },
    /// An evolution proposal, as `evolution.list` returns it, each time it
    /// is stored or changes.
    #[serde(rename = "proposal")]
    Proposal { proposal: Box<serde_json::Value> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_task_serializes_with_tag_and_task_field() {
        let task = crate::model::Task {
            id: "t1".into(),
            title: "T".into(),
            goal: "G".into(),
            criteria: vec![],
            verify: vec![],
            final_verify: vec![],
            checks: vec![],
            held_out: None,
            request: None,
            repo: "/r".into(),
            project_id: None,
            worktree: "/r-t".into(),
            worktree_removed: false,
            deliverables: vec![],
            visual_criteria: vec![],
            landed_sha: None,
            landed_at: None,
            diff_stat: None,
            report: None,
            report_at: None,
            lead_touch: None,
            follow_up_of: None,
            follow_ups: vec![],
            branch: "task/t".into(),
            base_sha: "abc".into(),
            base_ref: None,
            depends_on: vec![],
            paths: vec![],
            parent: None,
            status: crate::model::TaskStatus::Running,
            tier: crate::model::Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            budget_raises: 0,
            daily_budget_ok_day: None,
            assumptions: vec![],
            question_history: vec![],
            judged_findings: vec![],
            archived: false,
            pr_url: None,
            planned_tier: None,
            tier_fallback: None,
            variant: Default::default(),
            eval_set: None,
            eval_name: None,
            eval_check_cmd: None,
            source: None,
            eval_check: None,
            brief_check: Default::default(),
            queue: Default::default(),
            created_at: 1,
            updated_at: 1,
        };
        let ev = Event::Task {
            task: Box::new(task),
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["event"], "task");
        assert_eq!(v["task"]["id"], "t1");
        assert!(v["task"].get("questionHistory").is_none(), "{v}");
    }

    #[test]
    fn event_task_carries_the_question_history_and_who_asked() {
        use crate::model::*;
        let mut task: Task = serde_json::from_value(serde_json::json!({
            "id": "t1", "title": "", "goal": "", "criteria": [], "verify": [], "repo": "",
            "worktree": "", "branch": "", "baseSha": "", "status": "waiting", "tier": "standard",
            "createdAt": 0, "updatedAt": 0, "variant": {}
        }))
        .unwrap();
        let q = Question::new("Ok?", vec![], QuestionKind::Budget, AskedBy::Plan);
        task.question_history.push(AnsweredQuestion {
            question: q.text.clone(),
            options: vec![],
            kind: q.kind,
            asked_by: q.asked_by,
            asked_at: q.asked_at,
            answer: "yes".into(),
            answered_at: 9,
            answered_by: AnsweredBy::Owner,
        });
        task.question = Some(q);
        let v = serde_json::to_value(Event::Task {
            task: Box::new(task),
        })
        .unwrap();
        assert_eq!(v["task"]["question"]["askedBy"], "plan");
        assert_eq!(v["task"]["questionHistory"][0]["answeredBy"], "owner");
        assert_eq!(v["task"]["questionHistory"][0]["askedBy"], "plan");
    }

    #[test]
    fn event_log_serializes_with_camel_case_task_id() {
        let ev = Event::Log {
            task_id: "t1".into(),
            attempt: 2,
            line: "building...".into(),
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["event"], "log");
        assert_eq!(v["taskId"], "t1");
        assert_eq!(v["attempt"], 2);
    }
}
