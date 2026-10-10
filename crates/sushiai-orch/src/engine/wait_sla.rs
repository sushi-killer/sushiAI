use super::*;

/// How often the timer looks at waiting questions.
const SLA_CHECK: Duration = Duration::from_secs(60);
const HOUR_MS: i64 = 3_600_000;

/// Whether a reminder is due: one interval after the question was asked, or
/// after the last reminder for it. A restart after a long gap gives one
/// reminder, not one per missed interval.
pub(super) fn reminder_due(
    asked_at: i64,
    reminded_at: Option<i64>,
    every_hours: u32,
    now: i64,
) -> bool {
    if every_hours == 0 {
        return false;
    }
    let base = reminded_at.filter(|r| *r >= asked_at).unwrap_or(asked_at);
    now - base >= i64::from(every_hours) * HOUR_MS
}

/// Whether a question has waited `after_hours` (0 = never).
pub(super) fn default_due(asked_at: i64, after_hours: u32, now: i64) -> bool {
    after_hours > 0 && now - asked_at >= i64::from(after_hours) * HOUR_MS
}

impl App {
    /// One tick over the waiting questions an owner has not answered. A
    /// non-owner-only question past `waitDefaultAfterHours` takes its
    /// cautious option (recorded as a policy assumption); otherwise one past
    /// a `waitRemindEveryHours` interval wakes the overseer with a reminder.
    pub(super) async fn wait_sla_tick(&self, now: i64) {
        let (remind, default, auto_answer) = {
            let s = self.settings.read().unwrap();
            (
                s.wait_remind_every_hours,
                s.wait_default_after_hours,
                s.auto_answer,
            )
        };
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
                if let Some(answer) = cautious_choice(self, &task, &question) {
                    let evidence = format!(
                        "nobody answered in {default} h: the cautious choice, the owner can overturn it"
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
                // Only a question still waiting, and the same one, is reminded.
                let Ok(Some(mut fresh)) = self.store.load_task(&task.id) else {
                    continue;
                };
                if fresh.status != TaskStatus::Waiting
                    || fresh.question.as_ref().and_then(|q| q.asked_at) != Some(asked_at)
                {
                    continue;
                }
                fresh.queue.wait_reminded_at = Some(now);
                let _ = self.store.save_task(&fresh);
                // A busy chat queues the wake for the end of its turn.
                chat::wake_overseer(
                    self,
                    &task.repo,
                    crate::prompts::get("overseer_reminder")
                        .replace("{id}", &task.id)
                        .replace("{title}", &task.title),
                    false,
                    "reminder",
                    Some(&task.id),
                );
            }
        }
    }

    /// After a restart: wakes the overseer once per repo that has a waiting
    /// question the owner is not the only one to answer.
    pub(super) fn wake_overseer_for_waiting_after_restart(&self) {
        if !self.settings.read().unwrap().auto_answer {
            return;
        }
        let mut repos: Vec<String> = self
            .store
            .list_tasks()
            .unwrap_or_default()
            .into_iter()
            .filter(|t| {
                !t.archived
                    && t.status == TaskStatus::Waiting
                    && t.question.as_ref().is_some_and(|q| !owner_only(q.kind))
            })
            .map(|t| t.repo)
            .collect();
        repos.sort();
        repos.dedup();
        for repo in repos {
            chat::wake_overseer(
                self,
                &repo,
                crate::prompts::get("overseer_restart"),
                false,
                "restart",
                None,
            );
        }
    }

    /// The once-a-minute waiting-SLA check; ends with the daemon.
    pub(super) fn spawn_wait_sla_timer(&self) {
        let app = self.arc();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(SLA_CHECK);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if app.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                app.wait_sla_tick(now_ms()).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_800_000_000_000;
    const HALF_HOUR: i64 = HOUR_MS / 2;

    fn waiting_in(repo: &Path, kind: QuestionKind, options: &[&str]) -> Task {
        let mut task = task_with_status(TaskStatus::Waiting);
        task.repo = repo.to_string_lossy().to_string();
        let mut question = Question::new(
            "Which way?",
            options.iter().map(|o| o.to_string()).collect(),
            kind,
            AskedBy::Implement,
        );
        question.asked_at = Some(T0);
        task.question = Some(question);
        task
    }

    async fn calls_reach(repo: &Path, n: usize) {
        for _ in 0..400 {
            if chat::fake_overseer_calls(repo) >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("overseer calls never reached {n}");
    }

    #[test]
    fn a_reminder_is_due_one_interval_after_the_question_or_the_last_reminder() {
        assert!(!reminder_due(T0, None, 0, T0 + 99 * HOUR_MS));
        assert!(!reminder_due(T0, None, 1, T0 + HOUR_MS - 1));
        assert!(reminder_due(T0, None, 1, T0 + HOUR_MS));
        assert!(!reminder_due(T0, Some(T0 + HOUR_MS), 1, T0 + HOUR_MS + 1));
        // A reminder for an older question does not count.
        assert!(reminder_due(T0, Some(T0 - 5), 1, T0 + HOUR_MS));
        assert!(default_due(T0, 2, T0 + 2 * HOUR_MS));
        assert!(!default_due(T0, 0, T0 + 99 * HOUR_MS));
    }

    #[tokio::test]
    async fn a_question_waiting_three_and_a_half_hours_gets_three_hourly_reminders() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        {
            let mut s = app.settings.write().unwrap();
            s.auto_answer = true;
            s.wait_remind_every_hours = 1;
        }
        let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
        app.store.save_task(&task).unwrap();
        let repo_path = task.repo.clone();
        for step in 1..=7 {
            app.wait_sla_tick(T0 + step * HALF_HOUR).await;
            let due = [2, 4, 6].iter().filter(|s| **s <= step).count();
            calls_reach(&repo, due).await;
            chat::wait_idle(&app, &repo_path).await;
        }
        assert_eq!(chat::fake_overseer_calls(&repo), 3);
        // A tick right after a restart gives no burst for the missed hours.
        app.wait_sla_tick(T0 + 7 * HALF_HOUR + 1000).await;
        assert_eq!(chat::fake_overseer_calls(&repo), 3);
    }

    #[tokio::test]
    async fn the_default_answers_a_waiting_question_with_its_cautious_option_as_an_assumption() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        app.settings.write().unwrap().wait_default_after_hours = 2;
        let mut task = waiting_in(
            &repo,
            QuestionKind::PlanQuestion,
            &["rewrite it", "keep the old API (cautious)"],
        );
        app.store.save_task(&task).unwrap();
        let id = task.id.clone();
        let pending = Arc::new(StdMutex::new(None));
        let cancel = CancelToken::new();
        let waiter = {
            let (app, pending, cancel) = (app.clone(), pending.clone(), cancel.clone());
            tokio::spawn(async move {
                let id = task.id.clone();
                let answer =
                    wait_for_answer(&app, &id, &mut task, &pending, &cancel, &mut None).await;
                (answer, task)
            })
        };
        // The parked loop's control, as `spawn_task_loop` registers it.
        for _ in 0..200 {
            if pending.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        app.controls.lock().unwrap().insert(
            id.clone(),
            TaskControl {
                cancel: cancel.clone(),
                pending_answer: pending.clone(),
                pending_amend: Arc::new(StdMutex::new(None)),
                handle: tokio::spawn(async {}),
            },
        );
        app.wait_sla_tick(T0 + HOUR_MS).await;
        assert!(pending.lock().unwrap().is_some(), "not yet due");
        app.wait_sla_tick(T0 + 2 * HOUR_MS).await;
        let (answer, task) = waiter.await.unwrap();
        assert_eq!(answer.as_deref(), Some("keep the old API (cautious)"));
        assert_eq!(task.assumptions.len(), 1);
        let a = &task.assumptions[0];
        assert_eq!(
            (a.by.as_str(), a.kind),
            ("policy", Some(QuestionKind::PlanQuestion))
        );
        assert!(a.evidence.contains("2 h"), "{}", a.evidence);
        assert!(task.decisions.iter().any(|d| d.starts_with("Policy:")));
        assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
    }

    #[tokio::test]
    async fn the_default_never_answers_an_owner_only_kind_or_a_question_without_a_cautious_option()
    {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        app.settings.write().unwrap().wait_default_after_hours = 1;
        let owner = waiting_in(&repo, QuestionKind::Budget, &["raise (cautious)", "stop"]);
        let plain = waiting_in(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
        app.store.save_task(&owner).unwrap();
        app.store.save_task(&plain).unwrap();
        app.wait_sla_tick(T0 + 50 * HOUR_MS).await;
        for t in [&owner, &plain] {
            let t = app.store.load_task(&t.id).unwrap().unwrap();
            assert_eq!(t.status, TaskStatus::Waiting);
            assert!(t.question.is_some() && t.assumptions.is_empty());
        }
    }

    #[tokio::test]
    async fn a_restart_wakes_the_overseer_once_per_repo_for_non_owner_only_waits() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        app.settings.write().unwrap().auto_answer = true;
        let repo = dir.path().join("repo");
        let owner_repo = dir.path().join("owner");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&owner_repo).unwrap();
        for task in [
            waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]),
            waiting_in(&repo, QuestionKind::PlanQuestion, &["A"]),
            waiting_in(&owner_repo, QuestionKind::Permission, &["Allow once"]),
        ] {
            app.store.save_task(&task).unwrap();
        }
        app.wake_overseer_for_waiting_after_restart();
        calls_reach(&repo, 1).await;
        chat::wait_idle(&app, &repo.to_string_lossy()).await;
        assert_eq!(chat::fake_overseer_calls(&repo), 1);
        assert_eq!(chat::fake_overseer_calls(&owner_repo), 0);
    }

    #[tokio::test]
    async fn a_default_answer_to_a_planner_question_is_a_policy_assumption_and_leaves_nothing_behind(
    ) {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        app.settings.write().unwrap().wait_default_after_hours = 2;
        let mut task = task_with_status(TaskStatus::Drafting);
        task.repo = repo.to_string_lossy().to_string();
        app.store.save_task(&task).unwrap();
        let id = task.id.clone();
        let pending = Arc::new(StdMutex::new(None));
        let cancel = CancelToken::new();
        let waiter = {
            let (app, pending, cancel, id) =
                (app.clone(), pending.clone(), cancel.clone(), id.clone());
            tokio::spawn(async move {
                ask_plan_question(
                    &app,
                    &id,
                    "Which way?",
                    vec!["rewrite".into(), "keep it (reversible)".into()],
                    &pending,
                    &cancel,
                    &mut None,
                )
                .await
            })
        };
        for _ in 0..200 {
            if app
                .store
                .load_task(&id)
                .unwrap()
                .is_some_and(|t| t.question.is_some())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        app.controls.lock().unwrap().insert(
            id.clone(),
            TaskControl {
                cancel: cancel.clone(),
                pending_answer: pending.clone(),
                pending_amend: Arc::new(StdMutex::new(None)),
                handle: tokio::spawn(async {}),
            },
        );
        app.wait_sla_tick(now_ms() + 3 * HOUR_MS).await;
        assert_eq!(
            waiter.await.unwrap().as_deref(),
            Some("keep it (reversible)")
        );
        let task = app.store.load_task(&id).unwrap().unwrap();
        assert_eq!(task.assumptions.len(), 1);
        assert_eq!(task.assumptions[0].by, "policy");
        assert!(task.decisions.iter().any(|d| d.starts_with("Policy:")));
        assert!(task
            .decisions
            .iter()
            .all(|d| !d.starts_with("Orchestrator:")));
        // A second answer, by the owner, is not a policy one.
        app.agent_answers.lock().unwrap().remove(&id);
        let mut task = task;
        record_answer_for(&mut task, None, "x", false, None);
        assert!(task.assumptions.len() == 1);
    }

    #[tokio::test]
    async fn a_default_is_refused_when_the_loop_is_not_parked_any_more() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A (cautious)"]);
        app.store.save_task(&task).unwrap();
        app.controls.lock().unwrap().insert(
            task.id.clone(),
            TaskControl {
                cancel: CancelToken::new(),
                pending_answer: Arc::new(StdMutex::new(None)),
                pending_amend: Arc::new(StdMutex::new(None)),
                handle: tokio::spawn(async {}),
            },
        );
        let params =
            json!({"id": task.id, "answer": "A (cautious)", "source": "policy", "evidence": "e"});
        assert!(app.handle_task_answer(params).await.is_err());
        let after = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(after.status, TaskStatus::Waiting);
    }

    #[tokio::test]
    async fn a_pause_that_outlives_the_loop_is_served_after_a_restart_and_then_cleared() {
        let (app, dir) = test_app();
        let mut task = task_with_status(TaskStatus::Queued);
        task.repo = dir.path().join("gone").to_string_lossy().to_string();
        task.worktree = dir.path().join("gone-wt").to_string_lossy().to_string();
        task.queue.paused_until = Some(now_ms() + 700);
        app.store.save_task(&task).unwrap();
        let started = std::time::Instant::now();
        app.spawn_task_loop(task.id.clone(), true);
        tokio::time::sleep(Duration::from_millis(250)).await;
        let mid = app.store.load_task(&task.id).unwrap().unwrap();
        assert!(mid.queue.paused_until.is_some(), "still paused");
        for _ in 0..100 {
            let t = app.store.load_task(&task.id).unwrap().unwrap();
            if t.queue.paused_until.is_none() {
                assert!(started.elapsed() >= Duration::from_millis(600));
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the pause was never cleared");
    }
}
