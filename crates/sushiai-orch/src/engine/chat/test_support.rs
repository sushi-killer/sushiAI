use crate::engine::chat::proposals::{ChatProposal, ProposedTask};
use crate::engine::chat::sessions::push;
use crate::engine::chat::store::{load, save};
use crate::engine::chat::tools::{keep_tools, tools_in_line};
use crate::engine::chat::types::{ChatMode, ChatTool};
use crate::engine::state::App;
use crate::policy::routing::Harness;
use crate::policy::routing::Route;
use crate::policy::routing::Tier;
use crate::task::state::Task;
use crate::task::state::TaskStatus;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

/// Test support: points the Claude harness at a fake that records each
/// prompt it is given in `.overseer-calls` in its cwd (the repo) and replies
/// "Overseer note." Set once per process and never changed (the harness path is read from
/// the process environment by the engine).
#[cfg(test)]
pub(in crate::engine) fn fake_overseer_harness() -> &'static Path {
    static SCRIPT: std::sync::OnceLock<(tempfile::TempDir, PathBuf)> = std::sync::OnceLock::new();
    let (_, path) = SCRIPT.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake-claude.sh");
        // Written by a child `sh`, never through a descriptor of this process: a test thread
        // that forks while the file is open for writing makes the next exec of it fail with
        // ETXTBSY on Linux.
        let body = "#!/bin/sh\n[ -f .overseer-no-stdin ] && exec sleep 2\ncat >> .overseer-calls\nprintf '\\n=====\\n' >> .overseer-calls\n[ -f .overseer-slow ] && sleep 30\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"session_id\":\"s\",\"result\":\"Overseer note.\"}'\n";
        let status = std::process::Command::new("sh")
            .args(["-c", r#"printf '%s' "$1" > "$2" && chmod 755 "$2""#, "sh"])
            .arg(body)
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        std::env::set_var("ORCHD_CLAUDE_BIN", &path);
        (dir, path)
    });
    path
}

/// How many prompts the fake harness recorded in `repo`.
#[cfg(test)]
pub(in crate::engine) fn fake_overseer_calls(repo: &Path) -> usize {
    std::fs::read_to_string(repo.join(".overseer-calls"))
        .map(|t| t.matches("=====").count())
        .unwrap_or(0)
}

/// Whether the repo's current chat session holds a message with this text.
#[cfg(test)]
pub(in crate::engine) fn thread_has_text(app: &App, repo: &str, text: &str) -> bool {
    load(app, repo)
        .current()
        .messages
        .iter()
        .any(|m| m.role == "assistant" && m.text == text)
}

/// How many assistant messages of the repo's current chat session have this text.
#[cfg(test)]
pub(in crate::engine) fn thread_text_count(app: &App, repo: &str, text: &str) -> usize {
    load(app, repo)
        .current()
        .messages
        .iter()
        .filter(|m| m.role == "assistant" && m.text == text)
        .count()
}

/// Waits until no chat turn runs in `repo`.
#[cfg(test)]
pub(in crate::engine) async fn wait_idle(app: &App, repo: &str) {
    crate::engine::test_support::wait_async("the chat turn to end", || {
        !app.chat_turns.lock().unwrap().contains_key(repo)
    })
    .await;
}

pub(super) fn route(harness: Harness) -> Route {
    Route {
        id: "r".into(),
        label: "R".into(),
        harness,
        model: Some("m".into()),
        effort: None,
        profile_id: None,
        account_id: None,
        strength: None,
    }
}

pub(super) fn test_app() -> (Arc<App>, tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::new(
        dir.path().join("data"),
        dir.path().join("home"),
        "orchd".to_string(),
    )
    .unwrap();
    app.assume_harnesses_installed();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    (app, dir, repo.to_string_lossy().to_string())
}

/// Writes `session` into the repo's store as a finished turn would.
pub(super) fn converse(app: &App, repo: &str, text: &str, harness_session: &str) {
    let mut session = load(app, repo).current().clone();
    push(&mut session, "user", text, ChatMode::Chat);
    push(&mut session, "assistant", "ok", ChatMode::Chat);
    session.session_id = Some(harness_session.to_string());
    save(app, repo, &session);
}

pub(super) fn proposal_session(app: &App, repo: &str) -> String {
    let mut session = load(app, repo).current().clone();
    push(&mut session, "user", "plan it", ChatMode::Plan);
    push(&mut session, "assistant", "here", ChatMode::Plan);
    let task = |title: &str| ProposedTask {
        title: title.to_string(),
        goal: title.to_string(),
        ..Default::default()
    };
    let message = session.messages.last_mut().unwrap();
    message.proposal = Some(ChatProposal {
        feature: "Numbers".into(),
        tasks: vec![task("One"), task("Two"), task("Three")],
        ..Default::default()
    });
    let id = message.id.clone();
    save(app, repo, &session);
    id
}

pub(super) fn git_repo(repo: &str) {
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    ] {
        std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .unwrap();
    }
}

pub(super) fn block(json: &str) -> String {
    format!("```sushi-draft\n{json}\n```")
}

pub(super) fn stored_task(id: &str, title: &str) -> Task {
    crate::task::test_support::fixture(crate::task::TaskData {
        id: id.to_string(),
        title: title.to_string(),
        goal: "g".into(),
        criteria: vec![],
        verify: vec![],
        final_verify: vec![],
        checks: vec![],
        held_out: None,
        planner_tests: Default::default(),
        request: None,
        repo: "/repo".into(),
        project_id: None,
        worktree: "/wt".into(),
        worktree_removed: false,
        deliverables: vec![],
        visual_criteria: vec![],
        landed_sha: None,
        landed_at: None,
        unlanded: None,
        diff_stat: None,
        report: None,
        report_at: None,
        lead_touch: None,
        follow_up_of: None,
        follow_ups: vec![],
        branch: "b".into(),
        base_sha: "s".into(),
        base_ref: None,
        fresh_base: None,
        depends_on: vec![],
        addenda: vec![],
        paths: vec![],
        parent: None,
        status: TaskStatus::Running,
        tier: Tier::Standard,
        question: None,
        pending_amendment: None,
        delivery_checks: vec![],
        decisions: vec![],
        attempts: vec![],
        cost_usd: 0.0,
        budget_raises: 0,
        daily_budget_ok_day: None,
        assumptions: vec![],
        question_history: vec![],
        findings: vec![],
        archived: false,
        pr_url: None,
        pr_branch: None,
        pr_pending: false,
        pr_plan: None,
        overseer_woken: false,
        unreviewed: false,
        failed_count: 0,
        loop_state: Default::default(),
        planned_tier: None,
        planned_by_parent: false,
        tier_fallback: None,
        variant: Default::default(),
        eval_set: None,
        eval_name: None,
        eval_check_cmd: None,
        source: None,
        eval_check: None,
        brief_check: Default::default(),
        plan_gate: Default::default(),
        metrics: None,
        checklist: vec![],
        queue: Default::default(),
        created_at: 1,
        updated_at: 1,
    })
}

pub(super) fn tools_of(app: &App, repo: &str, harness: Harness, lines: &[&str]) -> Vec<ChatTool> {
    let mut seen = std::collections::HashSet::new();
    let mut list = Vec::new();
    for line in lines {
        keep_tools(
            &mut list,
            tools_in_line(app, repo, harness, line, &mut seen),
        );
    }
    list
}

pub(super) fn tool(name: &str, summary: &str) -> ChatTool {
    ChatTool {
        name: name.into(),
        summary: summary.into(),
    }
}
