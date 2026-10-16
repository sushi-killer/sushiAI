use super::process::Daemon;

pub fn run_file(daemon: &Daemon, task_id: &str, name: &str) -> String {
    attempt_file(daemon, task_id, "implement", name)
}

pub fn plan_file(daemon: &Daemon, task_id: &str, name: &str) -> String {
    attempt_file(daemon, task_id, "plan", name)
}

fn attempt_file(daemon: &Daemon, task_id: &str, stage: &str, name: &str) -> String {
    let task = daemon.request("task.get", serde_json::json!({"id": task_id}));
    let attempt = task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["stage"] == stage)
        .unwrap_or_else(|| panic!("no {stage} attempt: {task}"));
    std::fs::read_to_string(
        daemon
            .data_dir()
            .join("tasks")
            .join(task_id)
            .join("runs")
            .join(attempt["n"].as_u64().unwrap().to_string())
            .join(name),
    )
    .unwrap()
}
