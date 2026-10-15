use crate::policy::settings::ScopedCheck;
use crate::policy::variant::Variant;
use crate::prompts::brief_text;

use super::{
    build_plan_brief, build_plan_correction_brief, build_plan_retry_brief,
    build_subtask_plan_brief, build_subtask_plan_retry_brief, scoped_checks_block,
};

#[test]
fn every_plan_brief_says_measurement_is_an_orchd_check() {
    let v = Variant::default();
    let briefs = [
        build_plan_brief("r", &v, ""),
        build_subtask_plan_brief("r", &v, ""),
        build_plan_retry_brief("r", &v, ""),
        build_subtask_plan_retry_brief("r", &v, ""),
    ];
    for b in &briefs {
        for needle in [
            "unix sockets",
            "$ORCHD_BASE_SHA",
            "temporary worktree",
            "same benchmark",
            "before/after/delta",
            "exits non-zero unless the gain holds",
            "never committed",
        ] {
            assert!(b.contains(needle), "missing {needle}");
        }
    }
}

#[test]
fn the_implement_and_plan_briefs_tell_the_agent_to_search_with_rg() {
    assert!(brief_text("rules").contains("Search the code with `rg`"));
    let plan = build_plan_brief("req", &Variant::default(), "");
    assert!(plan.contains(brief_text("plan_search_rule")));
}

#[test]
fn the_plan_brief_always_asks_for_a_contract_and_a_final_verify_split() {
    let brief = build_plan_brief("r", &Variant::default(), "");
    assert!(brief.contains("-- check:"));
    assert!(brief.contains("\"finalVerify\":[]"));
}

#[test]
fn only_a_subtask_plan_brief_says_whole_repo_checks_run_at_the_parent() {
    let sentence = "run once at the parent, on the combined result";
    let sub = build_subtask_plan_brief("r", &Variant::default(), "");
    assert!(sub.contains(sentence));
    let top = build_plan_brief("r", &Variant::default(), "");
    assert!(!top.contains(sentence));
    assert!(top.contains("run once on the combined result"));
    assert!(top.contains("run at its parent"));
}

#[test]
fn the_plan_brief_says_what_visual_means() {
    let brief = build_plan_brief("r", &Variant::default(), "");
    assert!(brief.contains("proven by looking at an image"));
    assert!(brief.contains("a criterion checked by a command is never visual"));
}

#[test]
fn the_plan_brief_limits_visual_criteria_to_changed_screens_and_named_files() {
    let brief = build_plan_brief("r", &Variant::default(), "");
    assert!(brief.contains("only for a screen the task changes"));
    assert!(brief.contains("name its screenshot file (`artifacts/<name>.png`)"));
    assert!(brief.contains("one screenshot per changed screen"));
}

#[test]
fn the_plan_brief_lists_the_scoped_checks_of_the_tasks_repo() {
    let scoped = vec![
        ScopedCheck {
            repo: None,
            command: "npm run test:desktop".into(),
            paths: vec!["src/app/**".into(), "electron/**".into()],
        },
        ScopedCheck {
            repo: Some("/other".into()),
            command: "make slow".into(),
            paths: vec!["lib/**".into()],
        },
    ];
    let block = scoped_checks_block(&scoped, "/repo");
    assert!(block.contains(
        "`npm run test:desktop`: only when a changed file matches src/app/**, electron/**"
    ));
    assert!(!block.contains("make slow"));
    assert!(block.contains("do not put that command in `verify` or `finalVerify`"));
    assert_eq!(scoped_checks_block(&scoped[1..], "/repo"), "");
}

#[test]
fn the_plan_brief_asks_for_the_exact_screenshot_command_on_a_screen_change() {
    let brief = build_plan_brief("r", &Variant::default(), "");
    assert!(brief.contains("name the exact repo command that produces its screenshot evidence"));
}

#[test]
fn plan_brief_carries_the_request_verbatim_and_asks_for_the_sushi_plan_fence() {
    let brief = build_plan_brief(
        "add dark mode to the settings screen",
        &Variant::default(),
        "",
    );
    assert!(brief.contains("add dark mode to the settings screen"));
    assert!(brief.contains("sushi-plan"));
    assert!(brief.contains("AGENTS.md"));
}

#[test]
fn only_a_top_level_plan_brief_offers_subtasks() {
    let top = build_plan_brief("r", &Variant::default(), "");
    assert!(top.contains("\"subtasks\":[{\"key\""));
    assert!(top.contains("prefer 2-5 parts"));
    let part = build_subtask_plan_brief("r", &Variant::default(), "");
    assert!(!part.contains("subtasks"));
    assert!(build_subtask_plan_retry_brief("r", &Variant::default(), "").contains("previous reply"));
}

#[test]
fn the_plan_format_asks_for_coverage_and_a_check_per_subtask() {
    let top = build_plan_brief("r", &Variant::default(), "");
    assert!(
        top.contains("\"coverage\":[{\"requirement\":\"...\",\"criteria\":[0]}]"),
        "{top}"
    );
    assert!(
        top.contains("\"paths\":[\"src/...\"],\"check\":\"...\""),
        "{top}"
    );
    assert!(top.contains("merged into one task with an ordered checklist"));
    let part = build_subtask_plan_brief("r", &Variant::default(), "");
    assert!(part.contains("\"coverage\""));
}

#[test]
fn the_correction_brief_is_short_and_ends_with_the_plan_format() {
    let variant = Variant {
        batch_questions: true,
        ..Variant::default()
    };
    let fresh = build_plan_brief("Fix it", &variant, "");
    let fix = build_plan_correction_brief(" A vs B ", &variant, true);
    assert!(fix.starts_with("Your plan has a contradiction: A vs B"));
    assert!(fix.contains("keep every other part of your previous plan unchanged"));
    assert!(!fix.contains("Fix it") && !fix.contains("AGENTS.md"));
    let format = &fresh[fresh.find("## Report format").unwrap()..];
    assert!(fix.ends_with(format));
    assert!(fix.len() < fresh.len() / 2);
    let part = build_plan_correction_brief("x", &Variant::default(), false);
    assert!(!part.contains("subtasks") && part.contains("\"finalVerify\":[]"));
}

#[test]
fn plan_retry_brief_still_carries_the_original_request() {
    let retry = build_plan_retry_brief("add dark mode", &Variant::default(), "");
    assert!(retry.contains("add dark mode"));
    assert!(retry.contains("sushi-plan"));
    assert!(retry.to_lowercase().contains("previous reply"));
}

#[test]
fn plan_brief_places_past_work_between_request_and_instructions() {
    let variant = Variant::default();
    let section = "## Past work\n\nx\n";
    for brief in [
        build_plan_brief("Fix it", &variant, section),
        build_subtask_plan_brief("Fix it", &variant, section),
        build_plan_retry_brief("Fix it", &variant, section),
        build_subtask_plan_retry_brief("Fix it", &variant, section),
    ] {
        let request = brief.find("## Request").unwrap();
        let past = brief.find("## Past work").unwrap();
        let instructions = brief.find("## Instructions").unwrap();
        assert!(request < past && past < instructions);
    }
    assert!(!build_plan_brief("Fix it", &variant, "").contains("## Past work"));
}

#[test]
fn the_default_plan_brief_asks_for_a_recommended_option() {
    let brief = build_plan_brief("Fix it", &Variant::default(), "");
    assert!(brief.contains("\"recommended\""));
}

#[test]
fn the_plan_brief_asks_the_owner_only_the_closed_list_of_calls() {
    let variant = Variant {
        batch_questions: false,
        ..Variant::default()
    };
    let batched = build_plan_brief("Fix it", &Variant::default(), "");
    for brief in [build_plan_brief("Fix it", &variant, ""), batched] {
        for call in [
            "(1) a scope change against the request",
            "(2) money or a paid service",
            "(3) a destructive or irreversible action",
            "(4) the security model or a public API or contract change",
        ] {
            assert!(brief.contains(call), "missing {call}");
        }
        assert!(brief.contains("Anything else you settle yourself"));
    }
}

#[test]
fn every_plan_brief_asks_for_checks_held_out_and_tests() {
    let variant = Variant {
        batch_questions: false,
        ..Variant::default()
    };
    for brief in [
        build_plan_brief("Fix it", &variant, ""),
        build_subtask_plan_brief("Fix it", &variant, ""),
        build_plan_retry_brief("Fix it", &variant, ""),
        build_subtask_plan_retry_brief("Fix it", &variant, ""),
    ] {
        assert!(
            brief.contains("\"checks\":[{\"criterion\":0,\"run\":\"...\"}],\"heldOut\":{\"criterion\":0,\"run\":\"...\"},\"tests\":[{\"path\":\"...\",\"content\":\"...\"}]"),
            "{brief}"
        );
        assert!(brief.contains("must fail on the current code and pass once its criterion is met"));
        assert!(brief.contains("matches zero tests passes"));
        assert!(brief.contains("ORCHD_BASE_SHA"));
        assert!(brief.contains("already passes today proves nothing"));
        assert!(brief.contains("from the repository root"));
        assert!(brief.contains("the implementer never sees it"));
        assert!(brief.contains("the implementer cannot change them"));
        assert!(brief.find("\"checks\"").unwrap() < brief.find("\"questions\"").unwrap());
        assert!(!brief.contains("\"recommended\""));
    }
    assert!(build_plan_brief("Fix it", &variant, "").contains("\"subtasks\""));
    assert!(!build_subtask_plan_brief("Fix it", &variant, "").contains("\"subtasks\""));
    assert!(build_plan_retry_brief("Fix it", &variant, "")
        .ends_with(&build_plan_brief("Fix it", &variant, "")));
}

#[test]
fn plan_brief_with_grounded_checks_asks_for_part_checks() {
    for variant in [
        Variant::default(),
        Variant {
            batch_questions: true,
            ..Variant::default()
        },
    ] {
        let brief = build_plan_brief("Fix it", &variant, "");
        let subtasks = &brief[brief.find("\"subtasks\":[{").unwrap()..];
        assert!(subtasks.contains("\"checks\":[{\"criterion\":0,\"run\":\"...\"}],\"heldOut\":{\"criterion\":0,\"run\":\"...\"},\"finalVerify\":[]"), "{subtasks}");
        assert!(
            subtasks.contains("\"tests\":[{\"path\":\"...\",\"content\":\"...\"}]"),
            "{subtasks}"
        );
        assert!(subtasks.contains("\"screenshot\":\"\""), "{subtasks}");
        assert!(brief.contains("Draft `checks` and `heldOut` for each part"));
    }
}

#[test]
fn plan_brief_with_grounded_checks_asks_for_checks_and_a_held_out_check() {
    let variant = Variant::default();
    for brief in [
        build_plan_brief("Fix it", &variant, ""),
        build_subtask_plan_brief("Fix it", &variant, ""),
    ] {
        assert!(brief.contains("\"checks\":[{\"criterion\":0,\"run\":\"...\"}],\"heldOut\":{\"criterion\":0,\"run\":\"...\"}"), "{brief}");
        assert!(brief.contains("ORCHD_BASE_SHA"));
        assert!(brief.contains("the implementer never sees it"));
    }
}

#[test]
fn the_plan_brief_asks_for_double_dash_before_grep_patterns() {
    for brief in [
        build_plan_brief("x", &Variant::default(), ""),
        build_subtask_plan_brief("x", &Variant::default(), ""),
    ] {
        assert!(brief.contains("grep -qF -- '<pattern>'"), "{brief}");
    }
}
