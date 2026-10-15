use super::*;

#[test]
fn a_write_tool_call_is_held_for_one_owner_question_and_the_answer_goes_on_in_the_same_attempt() {
    let env = spawn();
    env.gate("hold", "");
    let id = env.create(&[]);
    env.start(&id);
    env.wait_run(1);
    let token = env.token(1);

    // A read tool runs without asking.
    let (decision, _) = verdict(&env.hook(&token, "mcp__fake__get_item", json!({})));
    assert_eq!(decision, "allow");
    assert_eq!(env.get(&id)["status"], "running");

    // Allow once: the call is held as a question, then allowed.
    let (result, waiting) = env.held(
        &token,
        &id,
        ("mcp__fake__send_note", json!({"text": "hello"})),
        "Allow once",
    );
    let question = &waiting["question"];
    assert_eq!(question["kind"], "permission");
    assert!(
        question["text"]
            .as_str()
            .unwrap()
            .starts_with("Allow mcp__fake__send_note for this task?"),
        "{question}"
    );
    assert_eq!(
        question["options"],
        json!(["Allow once", "Always for this repo", "Deny"])
    );
    assert_eq!(verdict(&result).0, "allow");
    let after = env.get(&id);
    assert_eq!(after["status"], "running", "the same attempt goes on");
    assert!(after["question"].is_null());
    assert_eq!(after["attempts"].as_array().unwrap().len(), 1);

    // A write to an outside service asks every time unless Always.
    let (result, _) = env.held(
        &token,
        &id,
        ("mcp__fake__send_note", json!({"text": "again"})),
        "Always for this repo",
    );
    assert_eq!(verdict(&result).0, "allow");
    let (decision, _) = verdict(&env.hook(&token, "mcp__fake__send_note", json!({})));
    assert_eq!(decision, "allow", "Always is not asked again");

    // Deny tells the agent, and the same call is not asked a second time.
    let (result, _) = env.held(
        &token,
        &id,
        ("mcp__fake__delete_item", json!({"id": 7})),
        "Deny",
    );
    let (decision, reason) = verdict(&result);
    assert_eq!(decision, "deny");
    assert!(
        reason.contains("The owner denied mcp__fake__delete_item"),
        "{reason}"
    );
    let (decision, _) = verdict(&env.hook(&token, "mcp__fake__delete_item", json!({})));
    assert_eq!(decision, "deny");
    assert_eq!(env.get(&id)["status"], "running");

    env.ungate("hold");
    let done = env.until(&id, "done");
    let lines = decisions(&done);
    assert!(lines.contains("Permission: allowed once: mcp__fake__send_note"));
    assert!(lines.contains("Permission: allowed for this repo: mcp__fake__send_note"));
    assert!(lines.contains("Permission: denied: mcp__fake__delete_item"));

    // The rule Always stored is inherited by the next task of the repo.
    env.gate("hold", "");
    let next = env.create(&[]);
    env.start(&next);
    env.wait_run(2);
    let (decision, _) = verdict(&env.hook(&env.token(2), "mcp__fake__send_note", json!({})));
    assert_eq!(decision, "allow");
    assert!(allow_list(&env, 2).contains(&"mcp__fake__send_note".to_string()));
    // A denial is for that run: delete_item asks again in the next task.
    let (result, _) = env.held(
        &env.token(2),
        &next,
        ("mcp__fake__delete_item", json!({})),
        "Allow once",
    );
    assert_eq!(verdict(&result).0, "allow");
    env.ungate("hold");
    env.until(&next, "done");
    env.daemon.shutdown_and_wait();
}

#[test]
fn a_refusal_after_the_run_asks_once_and_the_answer_reaches_the_next_attempt() {
    let env = spawn();
    env.gate("mode_1", "deny");
    let id = env.create(&[]);
    env.start(&id);
    let waiting = env.until(&id, "waiting");
    assert_eq!(env.runs(), 1, "no second attempt before the owner answers");
    assert_eq!(implement_attempts(&waiting), 1);
    assert_eq!(waiting["question"]["kind"], "permission");
    assert!(waiting["question"]["text"]
        .as_str()
        .unwrap()
        .starts_with("Allow mcp__fake__send_note for this task?"));

    env.daemon
        .request("task.answer", json!({"id": id, "answer": "Allow once"}));
    let done = env.until(&id, "done");
    assert_eq!(implement_attempts(&done), 2);
    assert!(
        allow_list(&env, 2).contains(&"mcp__fake__send_note".to_string()),
        "the attempt after the answer may use the tool"
    );
    env.daemon.shutdown_and_wait();
}

#[test]
fn the_same_refusal_again_never_starts_another_attempt() {
    let env = spawn();
    env.gate("mode", "deny");
    let id = env.create(&[]);
    env.start(&id);
    env.until(&id, "waiting");
    env.daemon
        .request("task.answer", json!({"id": id, "answer": "Deny"}));
    let failed = env.until(&id, "failed");
    assert_eq!(env.runs(), 2, "one attempt, the owner's answer, one more");
    assert_eq!(implement_attempts(&failed), 2);
    assert!(decisions(&failed).contains("refused again after the owner's answer"));
    env.daemon.shutdown_and_wait();
}

#[test]
fn a_no_tool_report_goes_to_the_owner_instead_of_a_retry() {
    let env = spawn();
    env.gate("mode_1", "notool");
    let id = env.create(&[]);
    env.start(&id);
    let waiting = env.until(&id, "waiting");
    assert_eq!(env.runs(), 1);
    let question = &waiting["question"];
    assert_eq!(question["kind"], "permission");
    assert!(question["text"]
        .as_str()
        .unwrap()
        .contains("no tool for move an Asana task"));
    // The connected Asana tool is off for the task: the owner may enable it.
    assert!(question["options"]
        .as_array()
        .unwrap()
        .contains(&json!("Enable asana for this task")));

    env.daemon.request(
        "task.answer",
        json!({"id": id, "answer": "I will do it: drop the criterion"}),
    );
    let done = env.until(&id, "done");
    assert!(decisions(&done).contains("the owner will do the part that needs move an Asana task"));
    env.daemon.shutdown_and_wait();
}

#[test]
fn a_no_tool_report_with_no_tool_to_enable_still_asks_the_owner() {
    let t = spawn();
    let mut settings = t.daemon.request("settings.get", json!({}));
    settings["chatTools"]
        .as_array_mut()
        .unwrap()
        .retain(|tool| tool["id"] != "asana");
    t.daemon
        .request("settings.set", json!({"settings": settings}));
    t.gate("mode_1", "notool");
    let id = t.create(&[]);
    t.start(&id);
    let waiting = t.until(&id, "waiting");
    let question = &waiting["question"];
    assert_eq!(question["kind"], "permission");
    assert_eq!(
        question["options"],
        json!(["I will do it: drop the criterion", "stop"])
    );
    assert!(!decisions(&waiting).contains("the owner will do the part"));
    t.daemon.shutdown_and_wait();
}
