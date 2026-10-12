use super::*;

/// How often the timer checks the clock.
const DIGEST_CHECK: Duration = Duration::from_secs(60);
/// Tasks touched within this window count as activity worth a digest.
const ACTIVITY_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;

/// Whether the morning digest is due: it is on, the local hour is the
/// configured one, and none ran on `today` yet.
pub(super) fn digest_due(
    digest_hour: Option<u8>,
    local_hour: u8,
    today: &str,
    last_day: Option<&str>,
) -> bool {
    digest_hour == Some(local_hour) && last_day != Some(today)
}

/// Local `("YYYY-MM-DD", hour)` for a Unix time in seconds.
fn local_day_and_hour(secs: i64) -> Option<(String, u8)> {
    let t = secs as libc::time_t;
    // SAFETY: `localtime_r` writes only into the zeroed `tm` we pass.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return None;
        }
        tm
    };
    Some((
        format!(
            "{:04}-{:02}-{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday
        ),
        tm.tm_hour as u8,
    ))
}

impl App {
    fn digest_days_path(&self) -> PathBuf {
        self.data_dir.join("digest-days.json")
    }

    /// One tick: when the digest hour is on, wakes the orchestrator chat in
    /// read-only mode for every repo with task activity in the last 24 h or
    /// a waiting task that has not had its digest today (the chat is per
    /// repo, so there is no single global turn). A repo's day is recorded
    /// only once its turn started, so a busy chat is retried on the next
    /// tick while the local hour still matches.
    fn digest_tick(&self, now_secs: i64) {
        let Some((today, hour)) = local_day_and_hour(now_secs) else {
            return;
        };
        let digest_hour = self.settings.read().unwrap().digest_hour;
        if digest_hour != Some(hour) {
            return;
        }
        let mut days: std::collections::BTreeMap<String, String> =
            std::fs::read_to_string(self.digest_days_path())
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
        let cutoff = now_ms() - ACTIVITY_WINDOW_MS;
        let mut repos: Vec<String> = self
            .store
            .list_tasks()
            .unwrap_or_default()
            .into_iter()
            .filter(|t| !t.archived && (t.updated_at >= cutoff || t.status == TaskStatus::Waiting))
            .map(|t| t.repo)
            .collect();
        repos.sort();
        repos.dedup();
        let mut changed = false;
        for repo in repos {
            let last = days.get(&repo).map(String::as_str);
            if digest_due(digest_hour, hour, &today, last)
                && chat::wake_overseer(
                    self,
                    &repo,
                    crate::prompts::get("digest"),
                    true,
                    "digest",
                    None,
                )
            {
                days.insert(repo, today.clone());
                changed = true;
            }
        }
        if changed {
            let _ = store::write_json_atomic(&self.digest_days_path(), &days);
        }
    }

    /// The once-a-minute digest check; ends with the daemon.
    pub(super) fn spawn_digest_timer(&self) {
        let app = self.arc();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(DIGEST_CHECK);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if app.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                app.digest_tick(now_ms() / 1000);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_is_due_only_at_its_hour_and_once_a_day() {
        assert!(digest_due(Some(8), 8, "2026-10-10", None));
        assert!(digest_due(Some(8), 8, "2026-10-10", Some("2026-10-09")));
        assert!(!digest_due(Some(8), 8, "2026-10-10", Some("2026-10-10")));
        assert!(!digest_due(Some(8), 9, "2026-10-10", None));
        assert!(!digest_due(None, 8, "2026-10-10", None));
    }

    #[tokio::test]
    async fn a_due_tick_lands_one_digest_in_the_chat_session_and_not_a_second() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_path = repo.to_string_lossy().to_string();
        let mut task = task_with_status(TaskStatus::Waiting);
        task.repo = repo_path.clone();
        task.updated_at = 1;
        app.store.save_task(&task).unwrap();
        let secs = 1_760_000_000;
        let (_, hour) = local_day_and_hour(secs).unwrap();
        app.settings.write().unwrap().digest_hour = Some(hour);
        app.digest_tick(secs);
        chat::wait_idle(&app, &repo_path).await;
        assert_eq!(chat::fake_overseer_calls(&repo), 1);
        assert!(chat::thread_has_text(&app, &repo_path, "Overseer note."));
        app.digest_tick(secs);
        chat::wait_idle(&app, &repo_path).await;
        assert_eq!(chat::fake_overseer_calls(&repo), 1);
        // Another day wakes it again.
        app.digest_tick(secs + 86_400);
        chat::wait_idle(&app, &repo_path).await;
        assert_eq!(chat::fake_overseer_calls(&repo), 2);
    }

    #[test]
    fn a_turn_end_is_logged_by_what_the_overseer_did() {
        let (app, _dir) = test_app();
        let end = |started: i64, task: &str, end: chat::TurnEnd| {
            chat::record_overseer_end(&app, "/r", "waiting", Some(task), started, Some(0.25), end)
        };
        // No answer: the reply is for the owner, with the turn's cost.
        end(0, "t1", chat::TurnEnd::Reply("Which branch?\nmore".into()));
        // The overseer answered during the turn: no owner note, still the cost.
        app.record_overseer(overseer::Entry::new("answered", "/r", Some("t2"), "q"));
        end(0, "t2", chat::TurnEnd::Reply("Answered.".into()));
        end(0, "t3", chat::TurnEnd::Stopped);
        end(0, "t4", chat::TurnEnd::Failed("harness exploded".into()));
        let log = overseer::read(&app.data_dir, None, None);
        let got: Vec<_> = log
            .iter()
            .rev()
            .map(|e| (e.kind.as_str(), e.text.as_str(), e.cost_usd))
            .collect();
        assert_eq!(
            got,
            [
                ("asked_owner", "Which branch?", Some(0.25)),
                ("answered", "q", None),
                ("done", "Answered.", Some(0.25)),
                ("skipped", "Stopped by you", Some(0.25)),
                ("failed", "harness exploded", Some(0.25)),
            ]
        );
    }

    #[tokio::test]
    async fn the_overseer_log_records_a_wake_and_its_digest_and_a_busy_skip() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_path = repo.to_string_lossy().to_string();
        let mut events = app.events_tx.subscribe();
        assert!(chat::wake_overseer(
            &app,
            &repo_path,
            "p".into(),
            true,
            "digest",
            None
        ));
        // The chat is busy now: a second wake is skipped and logged.
        assert!(!chat::wake_overseer(
            &app,
            &repo_path,
            "p".into(),
            false,
            "done",
            Some("t1")
        ));
        // The skipped wake is queued and fires when the digest turn ends.
        for _ in 0..400 {
            if chat::fake_overseer_calls(&repo) >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        chat::wait_idle(&app, &repo_path).await;
        let log = overseer::read(&app.data_dir, Some(&repo_path), None);
        let kinds: Vec<_> = log.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            ["asked_owner", "woke", "digest", "skipped", "woke"],
            "{log:?}"
        );
        let skipped = log.iter().find(|e| e.kind == "skipped").unwrap();
        assert_eq!(skipped.task_id.as_deref(), Some("t1"));
        assert!(skipped.text.contains("busy"), "{log:?}");
        assert_eq!(chat::fake_overseer_calls(&repo), 2);
        let mut seen = 0;
        while let Ok(event) = events.try_recv() {
            if matches!(event, Event::Overseer { .. }) {
                seen += 1;
            }
        }
        assert_eq!(seen, 5);
    }

    #[tokio::test]
    async fn a_shutdown_drops_queued_wakes_and_starts_no_new_turn() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_path = repo.to_string_lossy().to_string();
        assert!(chat::wake_overseer(
            &app,
            &repo_path,
            "p".into(),
            true,
            "digest",
            None
        ));
        assert!(!chat::wake_overseer(
            &app,
            &repo_path,
            "p".into(),
            false,
            "done",
            Some("t1")
        ));
        assert_eq!(app.deferred_wakes.lock().unwrap().len(), 1);
        app.shutdown();
        assert!(app.deferred_wakes.lock().unwrap().is_empty());
        chat::wait_idle(&app, &repo_path).await;
        assert!(!chat::wake_overseer(
            &app,
            &repo_path,
            "p".into(),
            false,
            "done",
            Some("t2")
        ));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(chat::fake_overseer_calls(&repo) <= 1);
        assert!(!app.chat_turns.lock().unwrap().contains_key(&repo_path));
    }

    #[test]
    fn an_answer_to_attempts_keep_failing_restarts_the_limit_pauses() {
        let mut task = task_with_status(TaskStatus::Waiting);
        task.queue.rate_pauses = 7;
        let q = Question::new(
            "Attempts keep failing",
            vec!["continue".into()],
            QuestionKind::AttemptsFailing,
            AskedBy::Implement,
        );
        record_answer(&mut task, Some(&q), "continue", AnsweredBy::Owner);
        assert_eq!(task.queue.rate_pauses, 0);
    }
}
