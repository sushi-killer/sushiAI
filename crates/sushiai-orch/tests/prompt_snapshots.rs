//! Every task-agent brief, rendered through its canonical owner on fixed fixtures and
//! compared byte for byte with its golden in
//! `tests/fixtures/prompt-snapshots/briefs/<kind>.txt`. A missing golden
//! fails; `UPDATE_PROMPT_SNAPSHOTS=1` writes the goldens instead.
//!
//! Uses public builders and model types. The fixtures hold no clock and no temp path.

use std::path::PathBuf;

#[path = "prompt_snapshots/fixtures.rs"]
mod fixtures;
#[path = "prompt_snapshots/rendering.rs"]
mod rendering;

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/prompt-snapshots/briefs")
}

/// The first byte where `a` and `b` differ, with a little context.
fn first_difference(golden: &str, rendered: &str) -> String {
    let at = golden
        .bytes()
        .zip(rendered.bytes())
        .position(|(g, r)| g != r)
        .unwrap_or(golden.len().min(rendered.len()));
    let around = |s: &str| {
        let from = at.saturating_sub(40);
        String::from_utf8_lossy(&s.as_bytes()[from.min(s.len())..(at + 40).min(s.len())])
            .into_owned()
    };
    format!(
        "differs at byte {at} (golden {} bytes, rendered {} bytes)\n  golden:   {:?}\n  rendered: {:?}",
        golden.len(),
        rendered.len(),
        around(golden),
        around(rendered)
    )
}

#[test]
fn every_brief_matches_its_golden() {
    let dir = golden_dir();
    let update = std::env::var("UPDATE_PROMPT_SNAPSHOTS").as_deref() == Ok("1");
    let renders = rendering::renders();
    let mut failures = Vec::new();
    for (kind, rendered) in &renders {
        assert!(!rendered.is_empty(), "{kind} rendered nothing");
        let path = dir.join(format!("{kind}.txt"));
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, rendered).unwrap();
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(golden) if &golden == rendered => {}
            Ok(golden) => failures.push(format!("{kind}: {}", first_difference(&golden, rendered))),
            Err(e) => failures.push(format!("{kind}: no golden at {} ({e})", path.display())),
        }
    }
    assert!(
        failures.is_empty(),
        "{} brief(s) differ from their goldens (UPDATE_PROMPT_SNAPSHOTS=1 rewrites them):\n{}",
        failures.len(),
        failures.join("\n")
    );

    // A golden no kind renders any more is stale.
    let stale: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".txt"))
        .filter(|name| {
            !renders
                .iter()
                .any(|(kind, _)| *name == format!("{kind}.txt"))
        })
        .collect();
    assert!(stale.is_empty(), "goldens without a brief kind: {stale:?}");
}
