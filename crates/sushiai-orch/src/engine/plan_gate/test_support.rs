pub(super) fn part(
    key: &str,
    deps: &[&str],
    paths: &[&str],
    check: &str,
) -> crate::engine::planning::draft::PlanSubtask {
    crate::engine::planning::draft::PlanSubtask {
        key: key.into(),
        title: key.to_uppercase(),
        request: format!("do {key}"),
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
        paths: paths.iter().map(|p| p.to_string()).collect(),
        check: check.into(),
        ..Default::default()
    }
}
