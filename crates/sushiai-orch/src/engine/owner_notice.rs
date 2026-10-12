//! The gate between an overseer reply and the owner: each decision reaches the
//! owner at most once. `verdict` is pure; it reads the overseer log, so the
//! memory survives a daemon restart and spans overseer sessions.
//!
//! State: an `asked_owner` or `digest` entry in `overseer.jsonl` carries the
//! decision keys it told the owner about (`Entry::decision`). A key is
//! `<task id>|<question kind>|<sorted options joined by 0x1f>`. A `skipped`
//! entry with the text [`DEFERRED`] carries the keys of a notice held for the
//! morning, and its detail is the notice.

use super::*;
use crate::overseer::Entry;

/// Log text of a notice held back by quiet hours.
pub(crate) const DEFERRED: &str = "Deferred to the morning digest (quiet hours)";
/// Log text of a reply dropped because the owner heard the question already.
pub(crate) const DUPLICATE: &str = "Not sent to the owner: already asked";
/// A notice is at most this many non-empty prose lines.
const MAX_NOTICE_LINES: usize = 3;
const HOUR_MS: i64 = 3_600_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Tell the owner.
    Notify,
    /// The owner heard this question within the reminder window.
    Duplicate,
    /// Quiet hours: hold it for the morning.
    Deferred,
    /// No question the turn was about is still open: the turn settled it.
    Resolved,
}

/// What a finished overseer turn is to do: the verdict and the decision keys
/// of the questions it covers.
#[derive(Debug, Clone)]
pub(crate) struct Gate {
    pub verdict: Verdict,
    pub keys: Vec<String>,
}

/// The settings the gate reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NoticeSettings {
    pub remind_hours: u32,
    /// Quiet window as (start, end) minutes of the local day.
    pub quiet: Option<(u16, u16)>,
}

impl NoticeSettings {
    pub(crate) fn from_settings(s: &Settings) -> NoticeSettings {
        NoticeSettings {
            remind_hours: s.wait_remind_every_hours,
            quiet: quiet_window(&s.quiet_hours).ok().flatten(),
        }
    }
}

/// Parses `HH:MM-HH:MM`; `""` is no window.
pub(crate) fn quiet_window(text: &str) -> Result<Option<(u16, u16)>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let bad = || "quietHours must be HH:MM-HH:MM (local time) or empty".to_string();
    let clock = |part: &str| -> Option<u16> {
        let (h, m) = part.split_once(':')?;
        let digits = |s: &str| s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit());
        if !digits(h) || !digits(m) {
            return None;
        }
        let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
        (h < 24 && m < 60).then_some(h * 60 + m)
    };
    let (from, to) = text.split_once('-').ok_or_else(bad)?;
    let (from, to) = (clock(from).ok_or_else(bad)?, clock(to).ok_or_else(bad)?);
    if from == to {
        return Err("quietHours start and end must differ; use \"\" to turn it off".to_string());
    }
    Ok(Some((from, to)))
}

pub(crate) fn in_quiet(window: Option<(u16, u16)>, minute: u16) -> bool {
    match window {
        None => false,
        Some((from, to)) if from < to => (from..to).contains(&minute),
        Some((from, to)) => minute >= from || minute < to,
    }
}

/// A question kind as its serialized name.
pub(crate) fn kind_name(kind: QuestionKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The sorted options of a question joined by 0x1f, each on one line.
pub(crate) fn option_part(options: &[String]) -> String {
    let mut options: Vec<String> = options
        .iter()
        .map(|o| o.replace(['\n', '\u{1f}'], " "))
        .collect();
    options.sort();
    options.join("\u{1f}")
}

/// The key of a question: task id, kind, sorted options and, when known, the
/// time it was asked, so a later question of the same shape is a new one. A
/// key from before `asked_at` was added has no fourth part and never matches.
pub(crate) fn decision_key(task_id: &str, question: &Question) -> String {
    let key = format!(
        "{task_id}|{}|{}",
        kind_name(question.kind),
        option_part(&question.options)
    );
    match question.asked_at {
        Some(at) => format!("{key}|{at}"),
        None => key,
    }
}

fn is_notice(entry: &Entry) -> bool {
    matches!(entry.kind.as_str(), "asked_owner" | "digest")
}

/// Whether `key` may reach the owner at `now` (ms) and local `minute` of the
/// day. A key that has a notice in the log less than the reminder window ago
/// (at any time when the window is 0) is a duplicate; otherwise quiet hours
/// defer it. Never `Resolved`: that is a turn-level verdict.
pub(crate) fn verdict(
    log: &[Entry],
    key: &str,
    now: i64,
    minute: u16,
    settings: &NoticeSettings,
) -> Verdict {
    let window = i64::from(settings.remind_hours) * HOUR_MS;
    let told = log.iter().any(|e| {
        is_notice(e) && e.decision_keys().any(|k| k == key) && (window == 0 || now - e.ts < window)
    });
    if told {
        Verdict::Duplicate
    } else if in_quiet(settings.quiet, minute) {
        Verdict::Deferred
    } else {
        Verdict::Notify
    }
}

/// The verdict of a turn over the questions still open after it: `Resolved`
/// when none is; `Notify` when any key is new; else the common verdict.
pub(crate) fn turn_verdict(
    log: &[Entry],
    keys: &[String],
    now: i64,
    minute: u16,
    settings: &NoticeSettings,
) -> Verdict {
    if keys.is_empty() {
        return Verdict::Resolved;
    }
    let all: Vec<Verdict> = keys
        .iter()
        .map(|k| verdict(log, k, now, minute, settings))
        .collect();
    [Verdict::Notify, Verdict::Deferred, Verdict::Duplicate]
        .into_iter()
        .find(|v| all.contains(v))
        .unwrap_or(Verdict::Duplicate)
}

/// A notice cut to its shape: at most three non-empty prose lines (the
/// decision, the options, the recommendation), then a trailing
/// ```sushi-draft block when there is one.
pub(crate) fn cap_notice(reply: &str) -> String {
    let (prose, block) = match reply.rfind("```sushi-draft") {
        Some(at) => (&reply[..at], Some(reply[at..].trim_end())),
        None => (reply, None),
    };
    let lines: Vec<&str> = prose
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .take(MAX_NOTICE_LINES)
        .collect();
    let mut out = lines.join("\n");
    if let Some(block) = block {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(block);
    }
    out
}

/// The local minute of the day of a Unix time in ms (noon when unknown).
pub(crate) fn minute_of_day(now_ms: i64) -> u16 {
    digest::local_day_and_time(now_ms / 1000)
        .map_or(720, |(_, h, m)| u16::from(h) * 60 + u16::from(m))
}

impl App {
    /// The questions a wake of `reason` is about, as (task id, asked_at): the
    /// wake's task, or for a restart every waiting non-owner-only task of the repo.
    pub(crate) fn wake_questions(
        &self,
        repo: &str,
        reason: &str,
        task_id: Option<&str>,
    ) -> Vec<(String, i64)> {
        let tasks: Vec<Task> = match (reason, task_id) {
            ("waiting" | "reminder", Some(id)) => self
                .store
                .load_task(id)
                .ok()
                .flatten()
                .into_iter()
                .collect(),
            ("restart", _) => self
                .store
                .list_tasks()
                .unwrap_or_default()
                .into_iter()
                .filter(|t| t.repo == repo)
                .filter(|t| t.question.as_ref().is_some_and(|q| !owner_only(q.kind)))
                .collect(),
            _ => Vec::new(),
        };
        tasks
            .into_iter()
            .filter(|t| t.status == TaskStatus::Waiting && !t.archived)
            .filter_map(|t| Some((t.id, t.question?.asked_at.unwrap_or(0))))
            .collect()
    }

    /// Keys of the questions in `about` that still wait on the same question.
    fn open_keys(&self, about: &[(String, i64)]) -> Vec<String> {
        let mut keys: Vec<String> = about
            .iter()
            .filter_map(|(id, asked_at)| {
                let task = self.store.load_task(id).ok().flatten()?;
                let question = task.question.as_ref()?;
                (task.status == TaskStatus::Waiting
                    && !task.archived
                    && question.asked_at.unwrap_or(0) == *asked_at)
                    .then(|| decision_key(id, question))
            })
            .collect();
        keys.sort();
        keys
    }

    /// Keys of every question waiting in `repo`, or in all repos for `None`.
    fn waiting_keys(&self, repo: Option<&str>) -> Vec<String> {
        let mut keys: Vec<String> = self
            .store
            .list_tasks()
            .unwrap_or_default()
            .iter()
            .filter(|t| repo.is_none_or(|r| t.repo == r))
            .filter(|t| t.status == TaskStatus::Waiting && !t.archived)
            .filter_map(|t| Some(decision_key(&t.id, t.question.as_ref()?)))
            .collect();
        keys.sort();
        keys
    }

    /// The overseer log, newest first.
    pub(crate) fn notice_log(&self) -> Vec<Entry> {
        crate::overseer::read(&self.data_dir, None, Some(crate::overseer::MAX_LINES))
    }

    /// The gate for a finished overseer turn; `None` for a wake the gate does
    /// not cover. The digest always reaches the owner and counts as the notice
    /// for every question waiting in the repo.
    pub(crate) fn overseer_gate(
        &self,
        repo: &str,
        reason: &str,
        about: &[(String, i64)],
    ) -> Option<Gate> {
        match reason {
            "digest" => Some(Gate {
                verdict: Verdict::Notify,
                keys: self.waiting_keys(Some(repo)),
            }),
            "waiting" | "reminder" | "restart" => {
                let keys = self.open_keys(about);
                let now = now_ms();
                let settings = NoticeSettings::from_settings(&self.settings.read().unwrap());
                let verdict = turn_verdict(
                    &self.notice_log(),
                    &keys,
                    now,
                    minute_of_day(now),
                    &settings,
                );
                Some(Gate { verdict, keys })
            }
            _ => None,
        }
    }

    /// With the digest off, the first tick after quiet hours posts each held
    /// notice once, if its question still waits and nothing told the owner since.
    pub(super) fn post_held_notices(&self, now: i64) {
        let open: std::collections::HashSet<String> = self.waiting_keys(None).into_iter().collect();
        // Newest first: a key told after its deferral is already settled.
        let log = self.notice_log();
        let mut settled = std::collections::HashSet::new();
        let mut held: Vec<(&Entry, Vec<String>)> = Vec::new();
        for entry in &log {
            let keys: Vec<String> = entry.decision_keys().map(str::to_string).collect();
            if is_notice(entry) {
                settled.extend(keys);
            } else if entry.kind == "skipped" && entry.text == DEFERRED {
                let live: Vec<String> = keys
                    .into_iter()
                    .filter(|k| open.contains(k) && !settled.contains(k))
                    .collect();
                if !live.is_empty() {
                    settled.extend(live.iter().cloned());
                    held.push((entry, live));
                }
            }
        }
        for (entry, keys) in held.into_iter().rev() {
            let text = entry.detail.clone().unwrap_or_else(|| entry.text.clone());
            if !chat::post_note(self, &entry.repo, &text) {
                continue;
            }
            let mut told = Entry::new("asked_owner", &entry.repo, entry.task_id.as_deref(), &text)
                .with_detail(&text)
                .with_decision(&keys);
            told.ts = now.max(told.ts);
            self.record_overseer(told);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_760_000_000_000;
    const MIN: i64 = 60_000;
    const NOON: u16 = 12 * 60;

    fn settings(remind_hours: u32, quiet: &str) -> NoticeSettings {
        NoticeSettings {
            remind_hours,
            quiet: quiet_window(quiet).unwrap(),
        }
    }

    fn noticed(key: &str, ts: i64) -> Entry {
        let mut e = Entry::new("asked_owner", "/repo", None, "Which way?")
            .with_decision(&[key.to_string()]);
        e.ts = ts;
        e
    }

    #[test]
    fn the_key_is_task_kind_and_sorted_options() {
        let q = |options: &[&str]| {
            Question::new(
                "Which?",
                options.iter().map(|o| o.to_string()).collect(),
                QuestionKind::PlanQuestion,
                AskedBy::Plan,
            )
        };
        let a = decision_key("t1", &q(&["B", "A"]));
        assert_eq!(a, decision_key("t1", &q(&["A", "B"])));
        assert_ne!(a, decision_key("t2", &q(&["A", "B"])));
        assert_ne!(a, decision_key("t1", &q(&["A"])));
        assert!(a.starts_with("t1|plan_question|"), "{a}");
    }

    #[test]
    fn a_later_question_on_the_same_task_is_a_new_decision() {
        let mut first = Question::new(
            "Which?",
            vec!["A".into(), "B".into()],
            QuestionKind::AttemptsFailing,
            AskedBy::Plan,
        );
        first.asked_at = Some(1_000);
        let mut second = first.clone();
        second.asked_at = Some(2_000);
        let (a, b) = (decision_key("t1", &first), decision_key("t1", &second));
        assert_ne!(a, b);
        let log = [noticed(&a, 5_000)];
        let settings = NoticeSettings {
            remind_hours: 0,
            quiet: None,
        };
        assert_eq!(verdict(&log, &a, 6_000, 0, &settings), Verdict::Duplicate);
        assert_eq!(verdict(&log, &b, 6_000, 0, &settings), Verdict::Notify);
        // A key logged before `asked_at` joined it never matches a new one.
        let old = noticed("t1|attempts_failing|A\u{1f}B", 5_000);
        assert_eq!(verdict(&[old], &b, 6_000, 0, &settings), Verdict::Notify);
    }

    #[test]
    fn quiet_hours_parse_wrap_midnight_and_refuse_nonsense() {
        assert_eq!(quiet_window("").unwrap(), None);
        let night = quiet_window("23:00-08:00").unwrap();
        assert!(in_quiet(night, 23 * 60) && in_quiet(night, 0) && in_quiet(night, 7 * 60 + 59));
        assert!(
            !in_quiet(night, 8 * 60) && !in_quiet(night, NOON) && !in_quiet(night, 22 * 60 + 59)
        );
        let day = quiet_window("13:00-14:30").unwrap();
        assert!(in_quiet(day, 13 * 60) && !in_quiet(day, 14 * 60 + 30) && !in_quiet(day, 12 * 60));
        for bad in [
            "25:00-08:00",
            "23:00",
            "23:00-08:60",
            "8:00-9:00",
            "09:00-09:00",
            "a-b",
            "23:00-",
        ] {
            assert!(quiet_window(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_key_told_inside_the_window_is_a_duplicate_and_after_it_a_reminder() {
        let log = [noticed("k", T0)];
        let s = settings(4, "");
        let at = |minutes: i64| verdict(&log, "k", T0 + minutes * MIN, NOON, &s);
        assert_eq!(at(4 * 60 - 1), Verdict::Duplicate);
        assert_eq!(at(4 * 60), Verdict::Notify);
        assert_eq!(verdict(&log, "other", T0, NOON, &s), Verdict::Notify);
        // With the reminder off, a told key is a duplicate for good.
        let off = settings(0, "");
        assert_eq!(
            verdict(&log, "k", T0 + 999 * 60 * MIN, NOON, &off),
            Verdict::Duplicate
        );
    }

    #[test]
    fn an_entry_without_a_decision_never_counts_and_a_digest_does() {
        let mut plain = Entry::new("asked_owner", "/repo", None, "old note");
        plain.ts = T0;
        let s = settings(4, "");
        assert_eq!(verdict(&[plain], "k", T0 + MIN, NOON, &s), Verdict::Notify);
        let mut digest = noticed("k", T0);
        digest.kind = "digest".into();
        assert_eq!(
            verdict(&[digest], "k", T0 + MIN, NOON, &s),
            Verdict::Duplicate
        );
        let mut skipped = noticed("k", T0);
        skipped.kind = "skipped".into();
        assert_eq!(
            verdict(&[skipped], "k", T0 + MIN, NOON, &s),
            Verdict::Notify
        );
    }

    #[test]
    fn a_notice_in_quiet_hours_is_deferred_but_a_duplicate_stays_a_duplicate() {
        let s = settings(4, "23:00-08:00");
        assert_eq!(verdict(&[], "k", T0, 23 * 60 + 30, &s), Verdict::Deferred);
        let log = [noticed("k", T0)];
        assert_eq!(
            verdict(&log, "k", T0 + MIN, 23 * 60 + 30, &s),
            Verdict::Duplicate
        );
        let keys = |k: &[&str]| k.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        assert_eq!(turn_verdict(&[], &[], T0, NOON, &s), Verdict::Resolved);
        assert_eq!(
            turn_verdict(&log, &keys(&["k", "new"]), T0 + MIN, NOON, &s),
            Verdict::Notify
        );
        assert_eq!(
            turn_verdict(&log, &keys(&["k"]), T0 + MIN, NOON, &s),
            Verdict::Duplicate
        );
    }

    #[test]
    fn a_notice_keeps_three_prose_lines_and_its_draft_block() {
        let reply = "Decision?\n\nOptions: A, B\nI recommend A.\nBecause of reasons.\nMore reasons.\n```sushi-draft\n{\"questions\":[]}\n```\n";
        assert_eq!(
            cap_notice(reply),
            "Decision?\nOptions: A, B\nI recommend A.\n\n```sushi-draft\n{\"questions\":[]}\n```"
        );
        assert_eq!(cap_notice("One line."), "One line.");
        assert_eq!(cap_notice(&"line\n".repeat(10)).lines().count(), 3);
    }

    /// A made-up night, replayed through the gate the way `run_turn` applies
    /// it: a question told at 21:50 (the log before the night), a restart, four
    /// base-retry answers, the told question still open, one new security
    /// question, and later the told question's reminder. Each step is
    /// (minutes after 21:50, keys still open after the turn).
    #[test]
    fn the_night_replay_tells_the_owner_once_per_real_decision() {
        let told = "task-a|agent_question|keep\u{1f}stop";
        let security = "task-sec|protected_path|approve\u{1f}refuse";
        let window = 4 * 60;
        let mut steps: Vec<(i64, Vec<&str>)> = vec![(30, vec![told])];
        steps.extend((0..4).map(|n| (40 + n * 10, vec![])));
        steps.extend([
            (90, vec![told]),
            (100, vec![security]),
            (window - 1, vec![told]),
            (window + 20, vec![told]),
            (window + 21, vec![told]),
        ]);

        let s = settings(4, "");
        let mut log = vec![noticed(told, T0)];
        let mut notices = Vec::new();
        for (at, open) in &steps {
            let now = T0 + at * MIN;
            let open: Vec<String> = open.iter().map(|k| k.to_string()).collect();
            let v = turn_verdict(&log, &open, now, NOON, &s);
            let kind = match v {
                Verdict::Notify => {
                    notices.push((*at, open[0].clone()));
                    "asked_owner"
                }
                Verdict::Resolved => "done",
                _ => "skipped",
            };
            let mut e = Entry::new(kind, "/repo", None, "turn").with_decision(&open);
            e.ts = now;
            log.push(e);
        }
        assert_eq!(
            notices,
            [(100, security.to_string()), (window + 20, told.to_string())],
            "the new question once; the told one once, after its window"
        );
        // Of the notices, one is for a new decision and one is the reminder.
        let (new_decisions, per_decision) = (1, notices.len() - 1);
        println!(
            "owner notifications per real decision: before 16/{new_decisions}, after {per_decision}/{new_decisions} (plus 1 reminder)"
        );
        assert_eq!(per_decision, 1);
    }

    #[test]
    fn the_night_replay_defers_in_quiet_hours() {
        let s = settings(4, "23:00-08:00");
        let key = ["task-sec|protected_path|approve".to_string()];
        assert_eq!(
            turn_verdict(&[], &key, T0, 23 * 60 + 30, &s),
            Verdict::Deferred
        );
        assert_eq!(turn_verdict(&[], &key, T0, 9 * 60, &s), Verdict::Notify);
    }
}
