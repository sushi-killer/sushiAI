#[cfg(test)]
use crate::engine::owner_notice;
#[cfg(test)]
use crate::overseer;
#[cfg(test)]
use crate::task::AnsweredBy;
#[cfg(test)]
use crate::task::AskedBy;
#[cfg(test)]
use crate::task::Question;
#[cfg(test)]
use crate::task::QuestionKind;

#[cfg(test)]
use crate::api::events::Event;
use crate::clock::now_ms;
#[cfg(test)]
use crate::engine::questions::answers::record_answer;
use crate::engine::state::App;
#[cfg(test)]
use crate::engine::test_support::task_with_status;
#[cfg(test)]
use crate::engine::test_support::test_app;
use crate::store;
use crate::task::TaskStatus;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

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

/// Local `("YYYY-MM-DD", hour, minute)` for a Unix time in seconds.
pub(super) fn local_day_and_time(secs: i64) -> Option<(String, u8, u8)> {
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
        tm.tm_min as u8,
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
        let Some((today, hour, _)) = local_day_and_time(now_secs) else {
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
            .map(|t| t.repo.clone())
            .collect();
        repos.sort();
        repos.dedup();
        let mut changed = false;
        for repo in repos {
            let last = days.get(&repo).map(String::as_str);
            if digest_due(digest_hour, hour, &today, last)
                && crate::engine::chat::wake::wake_overseer(
                    self,
                    &repo,
                    crate::engine::chat::wake::WakeRequest::new(
                        self.digest_prompt(&repo),
                        true,
                        "digest",
                        None,
                    ),
                )
            {
                days.insert(repo, today.clone());
                changed = true;
            }
        }
        if changed {
            let _ = store::files::write_json_atomic(&self.digest_days_path(), &days);
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
        let _harness = crate::engine::chat::test_support::fake_overseer_harness();
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_path = repo.to_string_lossy().to_string();
        let mut task = task_with_status(TaskStatus::Waiting);
        task.fixture_data().repo = repo_path.clone();
        task.fixture_data().updated_at = 1;
        app.store.save_task(&task).unwrap();
        let secs = 1_760_000_000;
        let (_, hour, _) = local_day_and_time(secs).unwrap();
        app.settings.write().unwrap().digest_hour = Some(hour);
        app.digest_tick(secs);
        crate::engine::chat::test_support::wait_idle(&app, &repo_path).await;
        assert_eq!(
            crate::engine::chat::test_support::fake_overseer_calls(&repo),
            1
        );
        assert!(crate::engine::chat::test_support::thread_has_text(
            &app,
            &repo_path,
            "Overseer note."
        ));
        app.digest_tick(secs);
        crate::engine::chat::test_support::wait_idle(&app, &repo_path).await;
        assert_eq!(
            crate::engine::chat::test_support::fake_overseer_calls(&repo),
            1
        );
        // Another day wakes it again.
        app.digest_tick(secs + 86_400);
        crate::engine::chat::test_support::wait_idle(&app, &repo_path).await;
        assert_eq!(
            crate::engine::chat::test_support::fake_overseer_calls(&repo),
            2
        );
    }

    #[test]
    fn a_turn_end_is_logged_by_what_the_overseer_did() {
        let (app, _dir) = test_app();
        let end = |started: i64, task: &str, end: crate::engine::chat::overseer::TurnEnd| {
            crate::engine::chat::overseer::record_overseer_end(
                &app,
                "/r",
                crate::engine::chat::overseer::OverseerRecord {
                    reason: "waiting",
                    task_id: Some(task),
                    started,
                    cost_usd: Some(0.25),
                    end,
                    gate: None,
                },
            )
        };
        // No answer: the reply is for the owner, with the turn's cost.
        end(
            0,
            "t1",
            crate::engine::chat::overseer::TurnEnd::Reply("Which branch?\nmore".into()),
        );
        // The overseer answered during the turn: no owner note, still the cost.
        app.record_overseer(overseer::Entry::new("answered", "/r", Some("t2"), "q"));
        end(
            0,
            "t2",
            crate::engine::chat::overseer::TurnEnd::Reply("Answered.".into()),
        );
        end(0, "t3", crate::engine::chat::overseer::TurnEnd::Stopped);
        end(
            0,
            "t4",
            crate::engine::chat::overseer::TurnEnd::Failed("harness exploded".into()),
        );
        // The turn settled its question itself: kept as `done`, never a notice.
        let gate = |verdict| {
            Some(owner_notice::Gate {
                verdict,
                keys: vec!["t5|agent_question|A".to_string()],
            })
        };
        crate::engine::chat::overseer::record_overseer_end(
            &app,
            "/r",
            crate::engine::chat::overseer::OverseerRecord {
                reason: "waiting",
                task_id: Some("t5"),
                started: 0,
                cost_usd: None,
                end: crate::engine::chat::overseer::TurnEnd::Reply(
                    "Retried it.\nSecond line.".into(),
                ),
                gate: gate(owner_notice::Verdict::Resolved),
            },
        );
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
                ("done", "Retried it.", None),
            ]
        );
        let resolved = &log[0];
        assert_eq!(
            resolved.detail.as_deref(),
            Some("Retried it.\nSecond line.")
        );
        assert_eq!(resolved.decision, None);
        // A duplicate and a deferred reply are logged as skipped with the reply behind them.
        for (verdict, text) in [
            (owner_notice::Verdict::Duplicate, owner_notice::DUPLICATE),
            (owner_notice::Verdict::Deferred, owner_notice::DEFERRED),
        ] {
            crate::engine::chat::overseer::record_overseer_end(
                &app,
                "/r",
                crate::engine::chat::overseer::OverseerRecord {
                    reason: "waiting",
                    task_id: Some("t5"),
                    started: 0,
                    cost_usd: None,
                    end: crate::engine::chat::overseer::TurnEnd::Reply("Keep going?".into()),
                    gate: gate(verdict),
                },
            );
            let last = &overseer::read(&app.data_dir, None, Some(1))[0];
            assert_eq!((last.kind.as_str(), last.text.as_str()), ("skipped", text));
            assert_eq!(last.detail.as_deref(), Some("Keep going?"));
            // Only the held notice remembers its decision.
            assert_eq!(
                last.decision.is_some(),
                verdict == owner_notice::Verdict::Deferred
            );
        }
    }

    #[tokio::test]
    async fn the_overseer_log_records_a_wake_and_its_digest_and_a_busy_skip() {
        let _harness = crate::engine::chat::test_support::fake_overseer_harness();
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_path = repo.to_string_lossy().to_string();
        let mut events = app.events_tx.subscribe();
        assert!(crate::engine::chat::wake::wake_overseer(
            &app,
            &repo_path,
            crate::engine::chat::wake::WakeRequest::new("p".into(), true, "digest", None)
        ));
        // The chat is busy now: a second wake is skipped and logged.
        assert!(!crate::engine::chat::wake::wake_overseer(
            &app,
            &repo_path,
            crate::engine::chat::wake::WakeRequest::new("p".into(), false, "done", Some("t1"))
        ));
        // The skipped wake is queued and fires when the digest turn ends.
        for _ in 0..400 {
            if crate::engine::chat::test_support::fake_overseer_calls(&repo) >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        crate::engine::chat::test_support::wait_idle(&app, &repo_path).await;
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
        assert_eq!(
            crate::engine::chat::test_support::fake_overseer_calls(&repo),
            2
        );
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
        let _harness = crate::engine::chat::test_support::fake_overseer_harness();
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_path = repo.to_string_lossy().to_string();
        assert!(crate::engine::chat::wake::wake_overseer(
            &app,
            &repo_path,
            crate::engine::chat::wake::WakeRequest::new("p".into(), true, "digest", None)
        ));
        assert!(!crate::engine::chat::wake::wake_overseer(
            &app,
            &repo_path,
            crate::engine::chat::wake::WakeRequest::new("p".into(), false, "done", Some("t1"))
        ));
        assert_eq!(app.deferred_wakes.lock().unwrap().len(), 1);
        app.shutdown();
        assert!(app.deferred_wakes.lock().unwrap().is_empty());
        crate::engine::chat::test_support::wait_idle(&app, &repo_path).await;
        assert!(!crate::engine::chat::wake::wake_overseer(
            &app,
            &repo_path,
            crate::engine::chat::wake::WakeRequest::new("p".into(), false, "done", Some("t2"))
        ));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(crate::engine::chat::test_support::fake_overseer_calls(&repo) <= 1);
        assert!(!app.chat_turns.lock().unwrap().contains_key(&repo_path));
    }

    #[test]
    fn an_answer_to_attempts_keep_failing_restarts_the_limit_pauses() {
        let mut task = task_with_status(TaskStatus::Waiting);
        task.fixture_data().queue.rate_pauses = 7;
        let q = Question::new(
            "Attempts keep failing",
            vec!["continue".into()],
            QuestionKind::AttemptsFailing,
            AskedBy::Implement,
            crate::clock::now_ms(),
        );
        record_answer(&mut task, Some(&q), "continue", AnsweredBy::Owner);
        assert_eq!(task.queue.rate_pauses, 0);
    }
}
