pub(crate) fn redact(line: &str, env: &std::collections::HashMap<String, String>) -> String {
    let mut values: Vec<_> = env.values().filter(|value| !value.is_empty()).collect();
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values.into_iter().fold(line.to_string(), |text, value| {
        text.replace(value, "[REDACTED]")
    })
}
