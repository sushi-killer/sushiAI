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
        // Owner-only questions that parked in quiet hours are noticed once they end.
        if !quiet {
            self.post_quiet_parked_notices(now);
        }
        let mut log: Option<Vec<crate::overseer::Entry>> = None;
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
                        fresh.queue.wait_reminded_at = Some(now);
                        reminded = true;
                    }
                }) {
                    eprintln!("orch: updating task {} failed: {e}", task.id);
                }
                if !reminded {
                    continue;
                }
                // A busy chat queues the wake for the end of its turn.
                chat::wake_overseer(
                    self,
                    &task.repo,
                    self.with_precedents(
                        crate::prompts::get("overseer_reminder")
                            .replace("{id}", &task.id)
                            .replace("{title}", &task.title),
                        &[&task],
                    ),
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
            .unwrap_or_else(|e| {
                eprintln!("orch: listing waiting tasks after restart failed: {e}");
                Vec::new()
            })
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
        let tasks = self.store.list_tasks().unwrap_or_default();
        for repo in repos {
            let waiting: Vec<&Task> = tasks
                .iter()
                .filter(|t| t.repo == repo && t.status == TaskStatus::Waiting && !t.archived)
                .collect();
            chat::wake_overseer(
                self,
                &repo,
                self.with_precedents(crate::prompts::get("overseer_restart"), &waiting),
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

    fn kinds(app: &App, repo: &str, kind: &str) -> usize {
        crate::overseer::read(&app.data_dir, Some(repo), None)
            .iter()
            .filter(|e| e.kind == kind)
            .count()
    }

    fn told(app: &App, task: &Task, ts: i64) {
        let key = owner_notice::decision_key(&task.id, task.question.as_ref().unwrap());
        let mut entry = crate::overseer::Entry::new("asked_owner", &task.repo, Some(&task.id), "Q")
            .with_decision(&[key]);
        entry.ts = ts;
        app.record_overseer(entry);
    }

    #[tokio::test]
    async fn a_restart_tells_the_owner_once_and_a_second_restart_says_nothing() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        app.settings.write().unwrap().auto_answer = true;
        app.settings.write().unwrap().wait_remind_every_hours = 4;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_str = repo.to_string_lossy().to_string();
        let mut task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
        task.question.as_mut().unwrap().asked_at = Some(now_ms());
        app.store.save_task(&task).unwrap();
        // The fake overseer answers nothing, so the question is still open: it is told.
        app.wake_overseer_for_waiting_after_restart();
        calls_reach(&repo, 1).await;
        chat::wait_idle(&app, &repo_str).await;
        assert!(chat::thread_has_text(&app, &repo_str, "Overseer note."));
        assert_eq!(kinds(&app, &repo_str, "asked_owner"), 1);
        // A new daemon reads the same overseer log and tells nothing again.
        let again = App::new(
            dir.path().join("data"),
            dir.path().join("home"),
            "orchd".to_string(),
        )
        .unwrap();
        again.assume_harnesses_installed();
        again.settings.write().unwrap().auto_answer = true;
        again.settings.write().unwrap().quiet_hours = String::new();
        again.wake_overseer_for_waiting_after_restart();
        calls_reach(&repo, 2).await;
        chat::wait_idle(&again, &repo_str).await;
        let notes = chat::thread_text_count(&again, &repo_str, "Overseer note.");
        assert_eq!(notes, 1, "no second message");
        assert_eq!(kinds(&again, &repo_str, "asked_owner"), 1);
        assert_eq!(kinds(&again, &repo_str, "skipped"), 1);
        let skipped = crate::overseer::read(&again.data_dir, Some(&repo_str), Some(1));
        assert_eq!(skipped[0].text, owner_notice::DUPLICATE);
    }

    #[tokio::test]
    async fn a_restart_with_only_told_questions_posts_nothing_and_logs_no_asked_owner() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        app.settings.write().unwrap().auto_answer = true;
        app.settings.write().unwrap().wait_remind_every_hours = 4;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_str = repo.to_string_lossy().to_string();
        let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
        app.store.save_task(&task).unwrap();
        told(&app, &task, now_ms() - 1000);
        let before = kinds(&app, &repo_str, "asked_owner");
        app.wake_overseer_for_waiting_after_restart();
        calls_reach(&repo, 1).await;
        chat::wait_idle(&app, &repo_str).await;
        assert!(!chat::thread_has_text(&app, &repo_str, "Overseer note."));
        assert_eq!(kinds(&app, &repo_str, "asked_owner"), before);
    }

    #[tokio::test]
    async fn a_turn_whose_question_is_gone_is_done_and_never_reaches_the_chat() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_str = repo.to_string_lossy().to_string();
        // The task is not waiting any more when the turn ends: it settled the question.
        let mut task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
        let id = task.id.clone();
        app.store.save_task(&task).unwrap();
        assert!(chat::wake_overseer(
            &app,
            &repo_str,
            "p".into(),
            false,
            "waiting",
            Some(&id)
        ));
        task.status = TaskStatus::Running;
        task.question = None;
        app.store.save_task(&task).unwrap();
        chat::wait_idle(&app, &repo_str).await;
        assert!(!chat::thread_has_text(&app, &repo_str, "Overseer note."));
        assert_eq!(kinds(&app, &repo_str, "asked_owner"), 0);
        assert_eq!(kinds(&app, &repo_str, "done"), 1);
    }

    #[tokio::test]
    async fn a_digest_restarts_the_reminder_window_and_the_reminder_comes_once_after_it() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        {
            let mut s = app.settings.write().unwrap();
            s.auto_answer = true;
            s.wait_remind_every_hours = 1;
        }
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_str = repo.to_string_lossy().to_string();
        let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
        app.store.save_task(&task).unwrap();
        // The owner heard the question 30 minutes after it was asked.
        let key = owner_notice::decision_key(&task.id, task.question.as_ref().unwrap());
        let mut digest =
            crate::overseer::Entry::new("digest", &repo_str, None, "Digest").with_decision(&[key]);
        digest.ts = T0 + HALF_HOUR;
        app.record_overseer(digest);
        // One interval after the question, the notice is only 30 minutes old: no wake.
        app.wait_sla_tick(T0 + HOUR_MS).await;
        assert_eq!(chat::fake_overseer_calls(&repo), 0);
        // One interval after the notice: one reminder, and none again at once.
        app.wait_sla_tick(T0 + HALF_HOUR + HOUR_MS).await;
        calls_reach(&repo, 1).await;
        chat::wait_idle(&app, &repo_str).await;
        app.wait_sla_tick(T0 + HALF_HOUR + HOUR_MS + 1000).await;
        assert_eq!(chat::fake_overseer_calls(&repo), 1);
    }

    #[tokio::test]
    async fn quiet_hours_start_no_reminder_and_the_held_notice_is_posted_once_after_them() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        {
            let mut s = app.settings.write().unwrap();
            s.auto_answer = true;
            s.wait_remind_every_hours = 1;
            // A window that holds the local minute of the tick, in any zone.
            let m = owner_notice::minute_of_day(T0 + 2 * HOUR_MS);
            let (from, to) = (m, (m + 120) % 1440);
            s.quiet_hours = format!(
                "{:02}:{:02}-{:02}:{:02}",
                from / 60,
                from % 60,
                to / 60,
                to % 60
            );
        }
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_str = repo.to_string_lossy().to_string();
        let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
        app.store.save_task(&task).unwrap();
        let key = owner_notice::decision_key(&task.id, task.question.as_ref().unwrap());
        app.wait_sla_tick(T0 + 2 * HOUR_MS).await;
        assert_eq!(
            chat::fake_overseer_calls(&repo),
            0,
            "no reminder wake in quiet hours"
        );
        // A notice held overnight, with the digest off, goes out once the window ends.
        let mut held = crate::overseer::Entry::new(
            "skipped",
            &repo_str,
            Some(&task.id),
            owner_notice::DEFERRED,
        )
        .with_detail("Which way?\nA or B\nI suggest A.")
        .with_decision(&[key]);
        held.ts = T0;
        app.record_overseer(held);
        app.wait_sla_tick(T0 + 2 * HOUR_MS).await;
        assert!(!chat::thread_has_text(
            &app,
            &repo_str,
            "Which way?\nA or B\nI suggest A."
        ));
        app.settings.write().unwrap().quiet_hours = String::new();
        app.wait_sla_tick(T0 + 3 * HOUR_MS).await;
        assert!(chat::thread_has_text(
            &app,
            &repo_str,
            "Which way?\nA or B\nI suggest A."
        ));
        assert_eq!(kinds(&app, &repo_str, "asked_owner"), 1);
        app.wait_sla_tick(T0 + 3 * HOUR_MS + 1000).await;
        assert_eq!(kinds(&app, &repo_str, "asked_owner"), 1, "once");
    }

    /// The `heldForOverseer` flag of each broadcast of task `id`, in order.
    fn held_flags(rx: &mut broadcast::Receiver<Event>, id: &str) -> Vec<bool> {
        let mut flags = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let Event::Task { task } = event {
                if task.id == id {
                    flags.push(task.question.as_ref().is_some_and(|q| q.held_for_overseer));
                }
            }
        }
        flags
    }

    #[tokio::test]
    async fn a_question_is_held_while_the_overseer_turn_runs_and_released_when_it_escalates() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        app.settings.write().unwrap().auto_answer = true;
        let repo = dir.path().join("repo");
        let owner_repo = dir.path().join("owner");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&owner_repo).unwrap();
        let plain = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
        let owner = waiting_in(&owner_repo, QuestionKind::Permission, &["Allow once"]);
        app.store.save_task(&plain).unwrap();
        app.store.save_task(&owner).unwrap();
        let mut rx = app.events_tx.subscribe();
        app.wake_overseer_for_waiting_after_restart();
        calls_reach(&repo, 1).await;
        chat::wait_idle(&app, &repo.to_string_lossy()).await;
        // The fake overseer answers nothing: the turn ended with the question open.
        let flags = held_flags(&mut rx, &plain.id);
        assert_eq!(flags, vec![true, false], "held, then released");
        let after = app.store.load_task(&plain.id).unwrap().unwrap();
        assert!(!after.question.unwrap().held_for_overseer);
        assert!(
            held_flags(&mut rx, &owner.id).is_empty(),
            "owner-only never changes"
        );
        let owner = app.store.load_task(&owner.id).unwrap().unwrap();
        assert!(!owner.question.unwrap().held_for_overseer);
    }

    fn held(app: &App, id: &str) -> bool {
        app.store
            .load_task(id)
            .unwrap()
            .unwrap()
            .question
            .unwrap()
            .held_for_overseer
    }

    #[tokio::test]
    async fn a_queued_wake_holds_the_question_at_once_and_a_wake_that_cannot_run_never_does() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        app.settings.write().unwrap().auto_answer = true;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
        app.store.save_task(&task).unwrap();
        let repo_str = task.repo.clone();
        // The chat is busy: the wake is queued and already the overseer's,
        // so the owner gets no notice for it.
        app.chat_turns
            .lock()
            .unwrap()
            .insert(repo_str.clone(), CancelToken::new());
        wake_orchestrator_for_waiting(&app, &task);
        assert!(held(&app, &task.id), "a queued wake holds the question");
        // The busy turn ends: the queued wake runs, holds, then releases.
        let mut rx = app.events_tx.subscribe();
        app.chat_turns.lock().unwrap().remove(&repo_str);
        chat::fire_deferred_wakes(&app, &repo_str);
        calls_reach(&repo, 1).await;
        chat::wait_idle(&app, &repo_str).await;
        assert_eq!(held_flags(&mut rx, &task.id), vec![true, false]);
        assert!(!held(&app, &task.id));
        // A daemon shutting down cannot wake it: nothing is held.
        app.shutting_down.store(true, Ordering::SeqCst);
        wake_orchestrator_for_waiting(&app, &task);
        assert!(!held(&app, &task.id));
    }

    #[tokio::test]
    async fn turning_auto_answer_on_wakes_the_overseer_for_a_question_parked_while_it_was_off() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
        app.store.save_task(&task).unwrap();
        let mut settings = app.settings.read().unwrap().clone();
        assert!(!settings.auto_answer);
        wake_orchestrator_for_waiting(&app, &task);
        assert_eq!(chat::fake_overseer_calls(&repo), 0, "off: no wake");

        settings.auto_answer = true;
        app.handle_settings_set(json!({ "settings": settings.clone() }))
            .await
            .unwrap();
        calls_reach(&repo, 1).await;
        chat::wait_idle(&app, &task.repo).await;
        // Saving settings that were already answering wakes nobody again.
        app.handle_settings_set(json!({ "settings": settings }))
            .await
            .unwrap();
        chat::wait_idle(&app, &task.repo).await;
        assert_eq!(chat::fake_overseer_calls(&repo), 1);
    }

    #[tokio::test]
    async fn a_hold_expires_after_ten_minutes_and_the_tick_releases_it() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let mut task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
        let q = task.question.as_mut().unwrap();
        q.held_for_overseer = true;
        q.held_at = Some(T0);
        assert!(q.held_now(T0 + HOLD_TTL_MS - 1));
        assert!(!q.held_now(T0 + HOLD_TTL_MS));
        app.store.save_task(&task).unwrap();
        let mut rx = app.events_tx.subscribe();
        app.wait_sla_tick(T0 + 1000).await;
        // Fresh in the test's eyes? The tick compares with the clock it is given.
        assert!(held(&app, &task.id), "still within ten minutes");
        app.wait_sla_tick(T0 + HOLD_TTL_MS + 1000).await;
        assert!(!held(&app, &task.id));
        assert_eq!(held_flags(&mut rx, &task.id), vec![false]);
        assert!(app
            .store
            .load_task(&task.id)
            .unwrap()
            .unwrap()
            .question
            .unwrap()
            .held_at
            .is_none());
    }

    #[tokio::test]
    async fn a_hung_overseer_turn_is_cancelled_logged_as_timed_out_and_releases_its_hold() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        {
            let mut s = app.settings.write().unwrap();
            s.auto_answer = true;
        }
        app.overseer_turn_timeout_ms
            .store(400, std::sync::atomic::Ordering::SeqCst);
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        // The fake harness sleeps 30 s when this file exists.
        std::fs::write(repo.join(".overseer-slow"), "").unwrap();
        let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
        app.store.save_task(&task).unwrap();
        let repo_str = task.repo.clone();
        let started = std::time::Instant::now();
        wake_orchestrator_for_waiting(&app, &task);
        assert!(held(&app, &task.id), "held while the turn runs");
        chat::wait_idle(&app, &repo_str).await;
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(!held(&app, &task.id), "released when the turn timed out");
        let log = crate::overseer::read(&app.data_dir, Some(&repo_str), None);
        assert!(
            log.iter()
                .any(|e| e.kind == "failed" && e.text.contains("timed out")),
            "{log:?}"
        );
    }

    #[tokio::test]
    async fn a_turn_releases_only_its_own_hold_and_not_when_another_wake_is_queued() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let mut task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
        let repo_str = task.repo.clone();
        let q = task.question.as_mut().unwrap();
        q.held_for_overseer = true;
        q.held_at = Some(2000);
        app.store.save_task(&task).unwrap();
        // An older turn's hold is not this question's current hold.
        app.release_holds(&repo_str, &[(task.id.clone(), 1000)]);
        assert!(held(&app, &task.id));
        // A queued wake for the task will hold it again: no flicker.
        app.deferred_wakes.lock().unwrap().insert(
            repo_str.clone(),
            vec![chat::test_deferred_wake("waiting", &task.id)],
        );
        app.release_holds(&repo_str, &[(task.id.clone(), 2000)]);
        assert!(held(&app, &task.id));
        app.deferred_wakes.lock().unwrap().clear();
        app.release_holds(&repo_str, &[(task.id.clone(), 2000)]);
        assert!(!held(&app, &task.id));
    }

    #[tokio::test]
    async fn holds_left_by_a_stopped_daemon_are_released_on_start() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let mut task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
        let q = task.question.as_mut().unwrap();
        q.held_for_overseer = true;
        q.held_at = Some(now_ms());
        app.store.save_task(&task).unwrap();
        app.release_stale_holds();
        assert!(!held(&app, &task.id));
    }

    #[test]
    fn the_flag_is_omitted_when_false_and_camel_case_when_true() {
        let mut q = Question::new("Which?", vec![], QuestionKind::AgentQuestion, AskedBy::Plan);
        assert!(serde_json::to_value(&q)
            .unwrap()
            .get("heldForOverseer")
            .is_none());
        q.held_for_overseer = true;
        assert_eq!(serde_json::to_value(&q).unwrap()["heldForOverseer"], true);
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
        record_answer_for(&mut task, None, "x", AnsweredBy::Owner, None, None);
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
        let until = now_ms() + 700;
        task.queue.paused_until = Some(until);
        app.store.save_task(&task).unwrap();
        app.spawn_task_loop(task.id.clone(), true);
        // The property is that the pause is served in full and then cleared. A wall-clock
        // sample of "still paused" would race the loop on a loaded machine: check the
        // moment of clearing against the pause's own end instead.
        let end = std::time::Instant::now() + crate::procs::testing::deadline();
        loop {
            let t = app.store.load_task(&task.id).unwrap().unwrap();
            if t.queue.paused_until.is_none() {
                assert!(now_ms() >= until, "the pause was cleared before it ended");
                return;
            }
            assert!(
                std::time::Instant::now() < end,
                "the pause was never cleared"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// A quiet window that holds the local minute of `at`, in any zone.
    fn window_around(at: i64) -> String {
        let m = owner_notice::minute_of_day(at);
        let to = (m + 120) % 1440;
        format!("{:02}:{:02}-{:02}:{:02}", m / 60, m % 60, to / 60, to % 60)
    }

    fn boundary_note(tasks: &[&Task]) -> String {
        let lines: Vec<String> = tasks
            .iter()
            .map(|t| format!("- Permission \u{2014} {} ({})", t.title, t.id))
            .collect();
        let noun = if tasks.len() == 1 {
            "question needs"
        } else {
            "questions need"
        };
        format!(
            "While quiet hours were on, {} {noun} your answer:\n{}",
            tasks.len(),
            lines.join("\n")
        )
    }

    /// Parks an owner-only question while quiet hours are on.
    fn park_in_quiet(app: &App, repo: &Path, options: &[&str]) -> Task {
        app.settings.write().unwrap().quiet_hours = window_around(now_ms());
        let task = waiting_in(repo, QuestionKind::Permission, options);
        app.store.save_task(&task).unwrap();
        wake_orchestrator_for_waiting(app, &task);
        task
    }

    fn fresh_app(dir: &Path) -> Arc<App> {
        let app = App::new(dir.join("data"), dir.join("home"), "orchd".to_string()).unwrap();
        app.assume_harnesses_installed();
        app.settings.write().unwrap().quiet_hours = String::new();
        app
    }

    #[tokio::test]
    async fn quiet_hours_owner_only_question_is_noticed_once_at_the_boundary() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_str = repo.to_string_lossy().to_string();
        let first = park_in_quiet(&app, &repo, &["Allow once", "Deny"]);
        let second = park_in_quiet(&app, &repo, &["Allow always", "Deny"]);
        let note = boundary_note(&[&first, &second]);
        let later = now_ms() + HOUR_MS;
        // The window is still active: nothing is posted.
        app.settings.write().unwrap().quiet_hours = window_around(later);
        app.wait_sla_tick(later).await;
        assert!(!chat::thread_has_text(&app, &repo_str, &note));
        assert_eq!(kinds(&app, &repo_str, "asked_owner"), 0);
        // The window ends: one note lists both, and a second tick adds none.
        app.settings.write().unwrap().quiet_hours = String::new();
        app.wait_sla_tick(later + 1000).await;
        assert_eq!(chat::thread_text_count(&app, &repo_str, &note), 1);
        assert_eq!(kinds(&app, &repo_str, "asked_owner"), 1);
        app.wait_sla_tick(later + 2000).await;
        assert_eq!(chat::thread_text_count(&app, &repo_str, &note), 1);
        assert_eq!(kinds(&app, &repo_str, "asked_owner"), 1);
    }

    #[tokio::test]
    async fn a_digest_that_told_the_question_suppresses_the_boundary_notice() {
        let _harness = chat::fake_overseer_harness();
        for digest_hour in [None, Some(7)] {
            let (app, dir) = test_app();
            app.settings.write().unwrap().digest_hour = digest_hour;
            let repo = dir.path().join("repo");
            std::fs::create_dir_all(&repo).unwrap();
            let repo_str = repo.to_string_lossy().to_string();
            let told_task = park_in_quiet(&app, &repo, &["Allow once", "Deny"]);
            let open_task = park_in_quiet(&app, &repo, &["Allow always", "Deny"]);
            let later = now_ms() + HOUR_MS;
            // A digest that ran after the first parked already told the owner.
            told(&app, &told_task, now_ms() + 1);
            app.settings.write().unwrap().quiet_hours = String::new();
            app.wait_sla_tick(later).await;
            assert!(chat::thread_has_text(
                &app,
                &repo_str,
                &boundary_note(&[&open_task])
            ));
            assert!(!chat::thread_has_text(
                &app,
                &repo_str,
                &boundary_note(&[&told_task, &open_task])
            ));
        }
    }

    #[tokio::test]
    async fn the_boundary_notice_survives_a_restart_and_a_full_overseer_log() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_str = repo.to_string_lossy().to_string();
        let task = park_in_quiet(&app, &repo, &["Allow once", "Deny"]);
        let note = boundary_note(&[&task]);
        // Appends past the cap in one write; reads keep the newest `MAX_LINES`.
        let flood = |app: &App| {
            use std::io::Write;
            let mut lines = String::new();
            for i in 0..crate::overseer::MAX_LINES + 10 {
                let entry =
                    crate::overseer::Entry::new("skipped", "/other", None, &format!("n{i}"));
                lines.push_str(&serde_json::to_string(&entry).unwrap());
                lines.push('\n');
            }
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(app.data_dir.join("overseer.jsonl"))
                .unwrap();
            file.write_all(lines.as_bytes()).unwrap();
        };
        // A restart and a full log lose nothing before the notice.
        let again = fresh_app(dir.path());
        flood(&again);
        let later = now_ms() + HOUR_MS;
        again.wait_sla_tick(later).await;
        assert_eq!(chat::thread_text_count(&again, &repo_str, &note), 1);
        // The notice entry is evicted and the daemon restarts: still once.
        flood(&again);
        let third = fresh_app(dir.path());
        third.wait_sla_tick(later + 1000).await;
        assert_eq!(chat::thread_text_count(&third, &repo_str, &note), 1);
    }

    #[tokio::test]
    async fn an_answered_owner_only_question_is_not_noticed_at_the_boundary() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_str = repo.to_string_lossy().to_string();
        let mut answered = park_in_quiet(&app, &repo, &["Allow once", "Deny"]);
        let mut archived = park_in_quiet(&app, &repo, &["Allow always", "Deny"]);
        let kept = park_in_quiet(&app, &repo, &["Allow here", "Deny"]);
        answered.status = TaskStatus::Running;
        answered.question = None;
        app.store.save_task(&answered).unwrap();
        archived.archived = true;
        app.store.save_task(&archived).unwrap();
        app.settings.write().unwrap().quiet_hours = String::new();
        // Parked outside quiet hours: nothing is recorded for it.
        let outside = waiting_in(&repo, QuestionKind::Permission, &["Allow outside", "Deny"]);
        app.store.save_task(&outside).unwrap();
        wake_orchestrator_for_waiting(&app, &outside);
        app.wait_sla_tick(now_ms() + HOUR_MS).await;
        assert!(chat::thread_has_text(
            &app,
            &repo_str,
            &boundary_note(&[&kept])
        ));
        assert_eq!(kinds(&app, &repo_str, "asked_owner"), 1);
    }
}
