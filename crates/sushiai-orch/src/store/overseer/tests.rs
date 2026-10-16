use super::*;
use crate::task::notes::overseer::{MAX_DETAIL_CHARS, MAX_TEXT_CHARS};
use crate::task::{Question, QuestionKind};

fn temp() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("overseer-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn text_is_the_first_line_cut_to_200_chars() {
    let e = Entry::new(
        "answered",
        "/r",
        Some("t1"),
        &format!("\n{}\nsecond", "x".repeat(500)),
        0,
    );
    assert_eq!(e.text.chars().count(), MAX_TEXT_CHARS);
    assert!(!e.text.contains('\n'));
}

#[test]
fn answered_reads_as_a_title_and_keeps_the_whole_exchange() {
    let cmd = "node --input-type=module -e \"import assert from 'node:assert/strict'\"";
    let mut q = Question::new(
        format!("{cmd} already fails on base 0123456: TOKEN=abc123 leaked output"),
        vec![],
        QuestionKind::PreexistingFailure,
        crate::task::AskedBy::Verify,
        crate::clock::now_ms(),
    );
    q.commands = vec![cmd.to_string(), "npm test".to_string()];
    let e = Entry::answered("/r", "t1", Some(&q), " keep all ", 0);
    assert_eq!(e.text, "Base check failing \u{2192} keep all");
    let d = e.detail.unwrap();
    assert!(d.starts_with("node --input-type") && d.ends_with("\n\nAnswer: keep all"));
    assert!(
        d.contains(&format!("{cmd}, npm test fail on base 0123456")) && !d.contains("TOKEN"),
        "{d}"
    );

    // A free question skips command lines and never starts with one.
    let q = Question::new(
        format!("{cmd}\nShould the retry limit be 3?"),
        vec![],
        QuestionKind::AgentQuestion,
        crate::task::AskedBy::Verify,
        crate::clock::now_ms(),
    );
    let e = Entry::answered("/r", "t1", Some(&q), "yes", 0);
    assert_eq!(e.text, "Should the retry limit be 3? \u{2192} yes");
    let only_code = Question::new(
        "npm test",
        vec![],
        QuestionKind::AgentQuestion,
        crate::task::AskedBy::Verify,
        crate::clock::now_ms(),
    );
    assert_eq!(
        Entry::answered("/r", "t", Some(&only_code), "ok", 0).text,
        "Question \u{2192} ok"
    );
}

#[test]
fn detail_is_capped_and_old_entries_without_it_still_load() {
    let e = Entry::new("done", "/r", None, "a", 0).with_detail(&"y".repeat(9000));
    assert_eq!(e.detail.as_ref().unwrap().chars().count(), MAX_DETAIL_CHARS);
    assert!(Entry::new("done", "/r", None, "a", 0)
        .with_detail("  ")
        .detail
        .is_none());
    let old: Entry =
        serde_json::from_str(r#"{"ts":1,"repo":"/r","kind":"woke","text":"x"}"#).unwrap();
    assert_eq!(old.detail, None);
    assert!(!serde_json::to_string(&old).unwrap().contains("detail"));
}

#[test]
fn an_old_line_without_a_decision_loads_and_has_no_keys() {
    let old: Entry = serde_json::from_str(
        r#"{"ts":1,"repo":"/r","kind":"asked_owner","taskId":"t","text":"x"}"#,
    )
    .unwrap();
    assert_eq!(old.decision, None);
    assert_eq!(old.decision_keys().count(), 0);
    assert!(!serde_json::to_string(&old).unwrap().contains("decision"));
    let keys = ["a|b|c".to_string(), "d|e|".to_string()];
    let e = Entry::new("asked_owner", "/r", None, "x", 0).with_decision(&keys);
    let back: Entry = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
    assert_eq!(back.decision_keys().collect::<Vec<_>>(), ["a|b|c", "d|e|"]);
}

#[test]
fn the_file_keeps_the_last_2000_lines() {
    let dir = temp();
    let lines = || std::fs::read_to_string(path(&dir)).unwrap().lines().count();
    let total = 2 * MAX_LINES + 5;
    for i in 0..total {
        append(&dir, &Entry::new("woke", "/r", None, &format!("n{i}"), 0)).unwrap();
        if i == MAX_LINES + 4 {
            // Reads stop at the newest `MAX_LINES` before the file is trimmed.
            let all = read(&dir, None, Some(MAX_LINES * 2));
            assert_eq!(all.len(), MAX_LINES);
            assert_eq!(all.last().unwrap().text, "n5");
        }
    }
    assert!(lines() <= 2 * MAX_LINES, "trimmed: {}", lines());
    let all = read(&dir, None, Some(MAX_LINES * 2));
    assert_eq!(all.len(), MAX_LINES);
    assert_eq!(all[0].text, format!("n{}", total - 1));
    assert_eq!(all.last().unwrap().text, format!("n{}", total - MAX_LINES));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_task_entry_is_appended_once_and_the_log_stays_private() {
    let dir = temp();
    let entry = Entry::new("done", "/r", Some("t1"), "a", 0);
    assert!(append_task_once(&dir, &entry).unwrap());
    assert!(!append_task_once(&dir, &entry).unwrap());
    assert_eq!(read(&dir, None, None).len(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path(&dir)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn read_filters_by_repo_newest_first_and_limits() {
    let dir = temp();
    for (repo, n) in [("/a", "1"), ("/b", "2"), ("/a", "3")] {
        append(&dir, &Entry::new("woke", repo, None, n, 0)).unwrap();
    }
    let a = read(&dir, Some("/a"), None);
    assert_eq!(
        a.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
        ["3", "1"]
    );
    assert_eq!(read(&dir, None, Some(1)).len(), 1);
    std::fs::remove_dir_all(&dir).ok();
}
