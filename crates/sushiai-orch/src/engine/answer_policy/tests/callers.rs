use super::*;

#[test]
fn a_command_retried_by_a_lead_overseer_or_chat_is_not_retried_by_rule() {
    for caller in [
        AnsweredBy::Lead,
        AnsweredBy::Overseer,
        AnsweredBy::Orchestrator,
        AnsweredBy::Owner,
    ] {
        let mut task = task_with_status(TaskStatus::Waiting);
        let first = crate::engine::verification::base::questions::pre_existing_question(
            &[VerifyOutcome {
                command: "a.sh".into(),
                code: Some(1),
                ..Default::default()
            }],
            &task.base_sha,
        );
        assert!(rule_answer(&task, &first, 1).is_some());
        record_answer(&mut task, Some(&first), PRE_EXISTING_RETRY, caller);
        let sha = task.base_sha.clone();
        record_owner_base_check(&mut task, &first.commands, &sha, PRE_EXISTING_RETRY, false);
        assert_eq!(
            rule_answer(&task, &first, 1).is_none(),
            caller != AnsweredBy::Owner
        );
        let mut new_command = first.clone();
        new_command.commands.push("b.sh".into());
        assert!(rule_answer(&task, &new_command, 1).is_some());
        task.fixture_data().base_sha = "1234567890".into();
        assert_eq!(
            rule_answer(&task, &first, 1).is_none(),
            caller != AnsweredBy::Owner
        );
    }
}
