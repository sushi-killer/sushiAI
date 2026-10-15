use crate::api::events::Event;
use crate::api::validation::validate_task_id;
use crate::clock::now_ms;
use crate::engine::answer_policy;
use crate::engine::attempts::pricing::price_resumed_attempt;
use crate::engine::availability::binary_found;
use crate::engine::availability::cheapest_available;
use crate::engine::availability::nearest_available;
use crate::engine::brief_check;
use crate::engine::brief_check::add_task_cost;
use crate::engine::chat_tools;
use crate::engine::cost::settle_interrupted_advisor;
use crate::engine::cost::settle_interrupted_review;
use crate::engine::cost::settle_unfinished_cost;
use crate::engine::decision::advance_after_failure;
use crate::engine::decision::consecutive_same_signature;
use crate::engine::decision::decide_after_failure;
use crate::engine::decision::drop_criterion;
use crate::engine::decision::failure_signature;
use crate::engine::decision::last_review_repeated;
use crate::engine::decision::FailureDecision;
use crate::engine::decision::FailureDecisionInput;
use crate::engine::decision_memory;
use crate::engine::deliverables;
use crate::engine::graph::parent::run_parent;
use crate::engine::graph::parent::ParentEnd;
use crate::engine::ladder;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::lifecycle::resume::resume_attempt;
use crate::engine::lines::no_planner_tier_line;
use crate::engine::lines::orchestrator_answer_line;
use crate::engine::lines::variant_route_line;
use crate::engine::owner_notice;
use crate::engine::past_work;
use crate::engine::plan_gate;
use crate::engine::planning::graph::check_subtasks;
use crate::engine::planning::graph::relay_links;
use crate::engine::planning::graph::serialise_overlapping;
use crate::engine::planning::graph::subtask_request;
use crate::engine::planning::graph::MAX_SUBTASKS;
use crate::engine::planning::questions::ask_plan_question;
use crate::engine::planning::subtasks::prepare_subtasks;
use crate::engine::questions::answers::record_answer_for;
use crate::engine::questions::exhausted::wait_for_exhausted_answer;
use crate::engine::questions::exhausted::MAX_ORCHESTRATOR_CONTINUES;
use crate::engine::questions::parking::park_for_answer;
use crate::engine::questions::parking::wait_for_answer;
use crate::engine::questions::reply::Reply;
use crate::engine::recovery;
use crate::engine::review::attempt_screenshots;
use crate::engine::review::evidence_scope;
use crate::engine::review::save_evidence;
use crate::engine::review::select_review_route;
use crate::engine::review::EvidenceScope;
use crate::engine::routing::resolve_variant;
use crate::engine::run_control::HookContext;
use crate::engine::run_control::TaskControl;
use crate::engine::state::App;
use crate::engine::test_claim;
#[cfg(test)]
use crate::engine::test_support;
use crate::engine::test_support::attempt_with_failure;
use crate::engine::test_support::git_in;
#[cfg(test)]
use crate::engine::test_support::init_git_repo;
#[cfg(test)]
use crate::engine::test_support::slow_helper_task;
use crate::engine::test_support::task_with_status;
use crate::engine::test_support::test_app;
#[cfg(test)]
use crate::engine::test_support::test_app_fitting_sandbox;
#[cfg(test)]
use crate::engine::test_support::test_app_parallel_one;
use crate::engine::verification::base::amendments::apply_pending_amendment;
use crate::engine::verification::base::answers::KEEP_ALL;
use crate::engine::verification::base::answers::PRE_EXISTING_DROP;
use crate::engine::verification::base::checks::broken_by_the_new_base;
use crate::engine::verification::base::checks::failing_on_base;
use crate::engine::verification::base::checks::failing_twice_on_base;
use crate::engine::verification::baseline::gated_checks;
use crate::engine::verification::context::record_check_outcomes;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::runner::run_one_verify_command;
use crate::harness;
use crate::harness::cancel::CancelToken;
use crate::harness::environment::login_shell_path;
use crate::harness::environment::merge_paths;
use crate::harness::guard::Guard;
use crate::harness::process::group_alive;
use crate::policy::keys::simple_hash;
use crate::policy::paths::glob_match;
use crate::policy::paths::matches_any_protected;
use crate::policy::pricing::Price;
use crate::policy::routing::route_strength;
use crate::policy::routing::Harness;
use crate::policy::routing::Route;
use crate::policy::routing::Tier;
use crate::policy::settings::SandboxMode;
use crate::policy::settings::ScopedCheck;
use crate::policy::settings::Settings;
use crate::policy::variant::Variant;
use crate::sandbox::TaskSandbox;
use crate::store::Store;
use crate::task::amendment::Amendment;
use crate::task::amendment::PendingAmendment;
use crate::task::checks::authority::Origin;
use crate::task::graph::has_cycle;
use crate::task::graph::wait_edges;
use crate::task::graph::waits_state;
use crate::task::graph::Waits;
use crate::task::is_command_check;
use crate::task::notes::RepoNote;
use crate::task::AnsweredBy;
use crate::task::AnsweredQuestion;
use crate::task::AskedBy;
use crate::task::Attempt;
use crate::task::AttemptStatus;
use crate::task::Backlog;
use crate::task::BacklogBucket;
use crate::task::Baseline;
use crate::task::Check;
use crate::task::Deliverable;
use crate::task::FailureKind;
use crate::task::Fingerprint;
use crate::task::MissingTool;
use crate::task::PlanFinding;
use crate::task::Question;
use crate::task::QuestionKind;
use crate::task::ReviewResult;
use crate::task::Severity;
use crate::task::Stage;
use crate::task::Task;
use crate::task::TaskStatus;
use crate::task::Verdict;
use crate::task::VerifyOutcome;
use crate::task::HOLD_TTL_MS;
use crate::task::PRE_EXISTING_RETRY;
use crate::verify::command::hook_budget_secs;
use crate::verify::diff::diff_hash;
use crate::verify::options::split_verify_commands;
use crate::verify::options::verify_options;
use crate::verify::output::classify_baseline;
use crate::verify::output::timed_out_tail;
use crate::verify::output::verify_tail;
use crate::verify::repository::whole_repo_suite;
use crate::verify::scoping::filter_scoped;
use crate::verify::scoping::filter_scoped_planned;
use serde_json::json;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::oneshot;

include!("availability.rs");
include!("cost.rs");
include!("decision.rs");
include!("for_owner.rs");
include!("graph.rs");
include!("lines.rs");
include!("core.rs");
include!("plan.rs");
include!("prompt_snapshots.rs");
include!("recovery.rs");
include!("review.rs");
include!("routing.rs");
include!("rpc_tasks.rs");
include!("stop.rs");
include!("verify.rs");
include!("worktrees.rs");

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    pub(super) fn the_login_shell_path_is_read_after_its_marker_and_merged_once() {
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("fake-shell");
        std::fs::write(
            &shell,
            "#!/bin/sh\necho 'welcome banner'\nPATH=/login/bin:/usr/bin\neval \"$2\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        // A script just written can fail to exec with ETXTBSY while a parallel
        // test forks (Linux); the spawn then reads as "no shell". Retry that.
        let login = (0..5).find_map(|_| {
            login_shell_path(shell.to_str().unwrap(), std::time::Duration::from_secs(5))
        });
        assert_eq!(login.as_deref(), Some("/login/bin:/usr/bin"));
        let merged = merge_paths("/usr/bin:/bin", login.as_deref().unwrap(), "/h");
        assert!(merged.starts_with("/usr/bin:/bin:/login/bin:"), "{merged}");
        assert!(merged.contains("/h/.cargo/bin"));
        assert_eq!(merged.matches("/usr/bin").count(), 1);
    }

    #[test]
    pub(super) fn a_shell_that_hangs_or_is_missing_gives_no_path() {
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("slow-shell");
        std::fs::write(&shell, "#!/bin/sh\nsleep 5\n").unwrap();
        std::fs::set_permissions(&shell, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let slow = login_shell_path(
            shell.to_str().unwrap(),
            std::time::Duration::from_millis(200),
        );
        assert!(slow.is_none());
        assert!(login_shell_path("/nope/shell", std::time::Duration::from_secs(1)).is_none());
        assert!(login_shell_path("", std::time::Duration::from_secs(1)).is_none());
    }
}
