/// What an attempt that ran out of time leaves for the next one.
pub(super) struct BudgetReport {
    pub handoff: String,
    pub summary: Option<String>,
}

/// Keep the report the stopped run already produced, or a factual handoff.
pub(super) fn report_after_budget(
    mins: f64,
    final_text: Option<&str>,
    changed_files: &[String],
) -> BudgetReport {
    let report = final_text.and_then(crate::engine::attempts::report::parse_report);
    BudgetReport {
        handoff: report
            .as_ref()
            .map(|r| r.handoff.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| fallback_handoff(mins, changed_files)),
        summary: report
            .map(|r| r.summary.trim().to_string())
            .filter(|s| !s.is_empty()),
    }
}

fn fallback_handoff(mins: f64, changed_files: &[String]) -> String {
    let files = if changed_files.is_empty() {
        "no file changed yet".to_string()
    } else {
        let shown: Vec<&str> = changed_files.iter().take(20).map(String::as_str).collect();
        let more = changed_files.len().saturating_sub(shown.len());
        let more = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        format!("changed files: {}{more}", shown.join(", "))
    };
    format!(
        "The attempt was stopped at its {mins} minute time budget before it reported; {files}. Continue from the working tree."
    )
}

#[cfg(test)]
mod tests {
    use super::{fallback_handoff, report_after_budget};

    #[test]
    fn the_fallback_names_the_budget_and_the_files() {
        let text = fallback_handoff(45.0, &["a.rs".into(), "b.rs".into()]);
        assert!(text.contains("45 minute"));
        assert!(text.contains("a.rs, b.rs"));
        assert!(fallback_handoff(0.05, &[]).contains("no file changed"));
    }

    #[test]
    fn the_existing_report_is_kept_without_another_turn() {
        let text = r#"```sushi-report
{"outcome":"partial","summary":" half done ","handoff":" finish the writer ","decisions":[],"question":""}
```"#;
        let report = report_after_budget(45.0, Some(text), &[]);
        assert_eq!(report.summary.as_deref(), Some("half done"));
        assert_eq!(report.handoff, "finish the writer");
    }

    #[test]
    fn a_summary_without_a_handoff_keeps_its_summary_and_names_the_changed_files() {
        let text = r#"```sushi-report
{"outcome":"partial","summary":"half done","decisions":[],"question":""}
```"#;
        let report = report_after_budget(0.05, Some(text), &["writer.rs".into()]);
        assert_eq!(report.summary.as_deref(), Some("half done"));
        assert!(report.handoff.contains("writer.rs"));
        assert!(report_after_budget(0.05, None, &[]).summary.is_none());
    }
}
