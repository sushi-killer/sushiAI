use crate::engine::answer_policy::default_choice;
use crate::engine::owner_notice;
use crate::engine::questions::wake::waiting_prompt;
use crate::engine::state::App;
use crate::policy::questions::owner_only;
use crate::policy::waits::default_due;
use crate::policy::waits::reminder_due;
use crate::task::TaskStatus;
use serde_json::json;

impl App {
    /// One tick over the waiting questions an owner has not answered. A
    /// non-owner-only question past `waitDefaultAfterHours` takes its
    /// default option (recorded as a policy assumption); otherwise one past
    /// a `waitRemindEveryHours` interval wakes the overseer with a reminder.
    pub(in crate::engine) async fn wait_sla_tick(&self, now: i64) {
        self.release_expired_holds(now);
        let (remind, default, auto_answer, digest_off, notice) = {
            let s = self.settings.read().unwrap();
            (
                s.wait_remind_every_hours,
                s.wait_default_after_hours,
                s.auto_answer,
                s.digest_hour.is_none(),
                owner_notice::NoticeSettings::from_settings(&s),
            )
        };
        let minute = owner_notice::minute_of_day(now);
        let quiet = owner_notice::in_quiet(notice.quiet, minute);
        // Without a digest, the notices held overnight go out once quiet hours end.
        if digest_off && !quiet {
            self.post_held_notices(now);
        }
        // Owner-only questions that parked in quiet hours are noticed once
        // they end, and the questions whose notices they held are the owner's.
        if !quiet {
            self.post_quiet_parked_notices(now);
            self.hand_over_deferred(now);
        }
        self.resume_cleared_waits(now).await;
        let mut log: Option<Vec<crate::task::notes::overseer::Entry>> = None;
        if remind == 0 && default == 0 {
            return;
        }
        for task in self.store.list_tasks().unwrap_or_default() {
            let Some(question) = task.question.clone() else {
                continue;
            };
            let Some(asked_at) = question.asked_at else {
                continue;
            };
            if task.archived || task.status != TaskStatus::Waiting || owner_only(question.kind) {
                continue;
            }
            if default_due(asked_at, default, now) {
                if let Some(answer) = default_choice(self, &task, &question) {
                    let evidence = format!(
                        "nobody answered in {default} h: the default choice, the owner can overturn it"
                    );
                    let params = json!({
                        "id": task.id,
                        "answer": answer,
                        "source": "policy",
                        "evidence": evidence,
                    });
                    if self.handle_task_answer(params).await.is_ok() {
                        continue;
                    }
                }
            }
            if auto_answer && reminder_due(asked_at, task.queue.wait_reminded_at, remind, now) {
                // A reminder that would only repeat a recent notice, or that
                // quiet hours hold, starts no wake; the next tick asks again.
                let key = owner_notice::decision_key(&task.id, &question);
                let log = log.get_or_insert_with(|| self.notice_log());
                if owner_notice::verdict(log, &key, now, minute, &notice)
                    != owner_notice::Verdict::Notify
                {
                    continue;
                }
                // Only a question still waiting, and the same one, is reminded.
                let mut reminded = false;
                if let Err(e) = self.update_task(&task.id, |fresh| {
                    if fresh.status == TaskStatus::Waiting
                        && fresh.question.as_ref().and_then(|q| q.asked_at) == Some(asked_at)
                    {
                        fresh.remind_waiting(now);
                        reminded = true;
                    }
                }) {
                    eprintln!("orch: updating task {} failed: {e}", task.id);
                }
                if !reminded {
                    continue;
                }
                // A busy chat queues the wake for the end of its turn.
                crate::engine::chat::wake::wake_overseer(
                    self,
                    &task.repo,
                    crate::engine::chat::wake::WakeRequest::new(
                        self.with_precedents(
                            waiting_prompt(self, "overseer_reminder", &task),
                            &[&task],
                        ),
                        false,
                        "reminder",
                        Some(&task.id),
                    ),
                );
            }
        }
    }
}
